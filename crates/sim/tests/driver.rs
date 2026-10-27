//! The cutting driver against the simulated Cameo (spec §5.5, §7.4): completion only after the
//! cutter reports idle, Pause at a cut boundary with the blade lifted, Stop within the paced
//! lead time, and interruptions reported as such. Behaviour here is the simulator's model of
//! public hypotheses, not validated machine behaviour.

mod common;

use std::time::Duration;

use cutcraft_device::driver::{self, Controls, Job, Outcome, Phase};
use cutcraft_device::gpgl::{self, CAMEO_1, EncodedPlan};
use cutcraft_path::plan_sheet;
use cutcraft_sim::{Clock, Profile, Scenario, Simulator, parity};

fn encoded() -> (cutcraft_path::CutPlan, EncodedPlan) {
    let project = common::project();
    let plan = plan_sheet(&project, &project.sheets[0].id).unwrap();
    let encoded = gpgl::encode(&plan, &CAMEO_1).unwrap();
    (plan, encoded)
}

/// A job whose estimates use the simulator's own timing, so the estimated blade can be held to
/// the simulated one.
fn job(plan: &EncodedPlan) -> Job<'_> {
    let profile = Profile::cameo_1();
    // The plan's speed setting, from its `!` command.
    let speed: f64 = String::from_utf8_lossy(&plan.bytes)
        .split('\u{3}')
        .find_map(|c| c.strip_prefix('!').and_then(|v| v.parse().ok()))
        .expect("a speed command");
    Job {
        plan,
        mm_per_s: speed * profile.speed_step_mm_s,
        travel_mm_s: profile.travel_mm_s,
        tool_change_s: 2.0 * profile.tool_change_s,
        point_s: profile.command_overhead_s,
        turn_s_per_rad: profile.turn_s_per_rad,
        units_per_mm: CAMEO_1.units_per_mm,
    }
}

fn simulator(scenario: &str, bytes: usize) -> Simulator {
    let scenario = Scenario::named(scenario, bytes).unwrap();
    Simulator::new(Profile::cameo_1(), Clock::Virtual, &scenario)
}

#[test]
fn a_sheet_completes_only_once_the_cutter_is_idle() {
    let (plan, encoded) = encoded();
    let sim = simulator("normal", encoded.bytes.len());
    let controls = Controls::default();
    let mut phases = Vec::new();
    let mut fractions = Vec::new();
    let outcome = driver::run(&mut sim.transport(), &sim, &job(&encoded), &controls, |p| {
        if phases.last() != Some(&p.phase) {
            phases.push(p.phase);
        }
        fractions.push(p.fraction);
    });
    assert!(matches!(outcome, Outcome::Completed { .. }), "{outcome:?}");
    assert_eq!(phases.first(), Some(&Phase::Sending));
    assert!(phases.contains(&Phase::Draining));
    assert!(fractions.iter().all(|f| *f < 1.0), "never 100% before idle");
    assert!(!sim.snapshot().tool_down);
    let report = sim.with_machine(|m| parity::check(&plan, m, CAMEO_1.quantization_budget_mm()));
    assert!(report.matches, "{report:#?}");
}

#[test]
fn pause_waits_for_a_cut_boundary_and_resume_finishes_the_sheet() {
    let (plan, encoded) = encoded();
    let sim = simulator("normal", encoded.bytes.len());
    let controls = Controls::default();
    let mut paused_at = None;
    let mut paused_polls = 0;
    let outcome = driver::run(
        &mut sim.transport(),
        &sim,
        &job(&encoded),
        &controls,
        |p| match p.phase {
            Phase::Sending if p.passes_sent == 2 && paused_at.is_none() => controls.pause(),
            Phase::Paused => {
                let snapshot = sim.snapshot();
                assert!(!snapshot.tool_down, "the blade is lifted while paused");
                paused_at.get_or_insert(p.passes_sent);
                paused_polls += 1;
                if paused_polls == 5 {
                    controls.resume();
                }
            }
            _ => {}
        },
    );
    assert!(matches!(outcome, Outcome::Completed { .. }), "{outcome:?}");
    let paused_at = paused_at.expect("it paused");
    assert!(paused_at >= 2, "paused at a cut boundary after the request");
    let report = sim.with_machine(|m| parity::check(&plan, m, CAMEO_1.quantization_budget_mm()));
    assert!(
        report.matches,
        "pausing changes nothing that is cut: {report:#?}"
    );
}

#[test]
fn stop_sends_nothing_more_and_the_cutter_goes_idle_within_the_lead() {
    let (_, encoded) = encoded();
    let sim = simulator("normal", encoded.bytes.len());
    let controls = Controls::default();
    let mut requested = None;
    let outcome = driver::run(&mut sim.transport(), &sim, &job(&encoded), &controls, |p| {
        if p.phase == Phase::Sending && p.passes_sent == 3 && requested.is_none() {
            requested = Some((sim.snapshot().now_s, sim.received()));
            controls.stop();
        }
    });
    let Outcome::Stopped { idle, passes_sent } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(idle);
    assert!(passes_sent < encoded.passes.len());
    let (at, received) = requested.unwrap();
    let snapshot = sim.snapshot();
    assert!(!snapshot.tool_down, "the simulated blade is up after Stop");
    // After Stop only the blade-up request and status queries (2 bytes each) are sent.
    assert!(sim.received() > received);
    let last_down_end = sim.with_machine(|m| {
        m.trace()
            .iter()
            .filter(|s| s.tool_down)
            .map(|s| s.end_s)
            .fold(0.0, f64::max)
    });
    // Queued motion is at most the lead plus the command sent last (up to `max_command_s` of
    // travel, plus its turns).
    assert!(
        last_down_end - at <= driver::LEAD_S + CAMEO_1.max_command_s + 0.25,
        "cutting continued {:.2} s after Stop",
        last_down_end - at
    );
}

#[test]
fn faults_are_reported_as_interruptions_or_not_loaded() {
    let (_, encoded) = encoded();
    for (scenario, expect) in [
        ("no-media", "not-loaded"),
        ("disconnect-mid-cut", "interrupted"),
        ("power-off-mid-cut", "interrupted"),
        ("unload-mid-cut", "interrupted"),
        ("timeout-after-delivery", "interrupted"),
        ("stall", "interrupted"),
        ("slow-status", "completed"),
        ("stale-status", "completed"),
    ] {
        let sim = simulator(scenario, encoded.bytes.len());
        let outcome = driver::run(
            &mut sim.transport(),
            &sim,
            &job(&encoded),
            &Controls::default(),
            |_| {},
        );
        let got = match outcome {
            Outcome::Completed { .. } => "completed",
            Outcome::NotLoaded => "not-loaded",
            Outcome::Refused { .. } => "refused",
            Outcome::Interrupted { .. } => "interrupted",
            Outcome::Stopped { .. } => "stopped",
        };
        assert_eq!(got, expect, "{scenario}: {outcome:?}");
    }
}

#[test]
fn stop_while_draining_abandons_the_sheet_and_lifts_the_tool() {
    let (_, encoded) = encoded();
    for stop_on_last_write in [false, true] {
        let sim = simulator("normal", encoded.bytes.len());
        let controls = Controls::default();
        let mut requested = false;
        let outcome = driver::run(&mut sim.transport(), &sim, &job(&encoded), &controls, |p| {
            let trigger = if stop_on_last_write {
                p.phase == Phase::Sending && p.sent_bytes == p.total_bytes
            } else {
                p.phase == Phase::Draining
            };
            if trigger && !requested {
                requested = true;
                controls.stop();
            }
        });
        assert!(requested, "the test must reach the requested stop point");
        assert!(
            matches!(outcome, Outcome::Stopped { idle: true, .. }),
            "{outcome:?}"
        );
        assert!(!sim.snapshot().tool_down);
    }
}

#[test]
fn a_ready_reply_just_after_the_firmware_query_does_not_start_an_unloaded_cut() {
    let (_, encoded) = encoded();
    let sim = simulator("no-media", encoded.bytes.len());
    let mut transport = sim.transport();
    let firmware = driver::query_firmware(&mut transport, Duration::from_secs(2)).unwrap();
    assert_eq!(firmware, "CAMEO V1.20");
    assert_eq!(
        driver::query_status(&mut transport, Duration::from_secs(2)).unwrap(),
        driver::DeviceStatus::Ready,
        "the simulator reproduces the observed transient"
    );
    let received = sim.received();
    let outcome = driver::run(
        &mut transport,
        &sim,
        &job(&encoded),
        &Controls::default(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::NotLoaded);
    // Only status queries (ESC ENQ) followed the firmware query.
    assert_eq!((sim.received() - received) % 2, 0);
    assert!(sim.with_machine(|m| m.trace().is_empty() && m.faults().is_empty()));
}

#[test]
fn a_loaded_cut_waits_for_status_to_settle_before_sending() {
    let (_, encoded) = encoded();
    let sim = simulator("normal", encoded.bytes.len());
    let mut transport = sim.transport();
    driver::query_firmware(&mut transport, Duration::from_secs(2)).unwrap();
    let start = sim.snapshot().now_s;
    let mut first_send = None;
    let outcome = driver::run(
        &mut transport,
        &sim,
        &job(&encoded),
        &Controls::default(),
        |p| {
            if p.sent_bytes > 0 && first_send.is_none() {
                first_send = Some(sim.snapshot().now_s);
            }
        },
    );
    assert!(matches!(outcome, Outcome::Completed { .. }), "{outcome:?}");
    let waited = first_send.unwrap() - start;
    assert!(waited >= driver::SETTLE_S, "sent after {waited:.2} s");
}

#[test]
fn stop_while_status_settles_sends_nothing() {
    let (_, encoded) = encoded();
    let sim = simulator("normal", encoded.bytes.len());
    let controls = Controls::default();
    let start = sim.snapshot().now_s;
    // The driver reports nothing while status settles, so Stop is asked before it runs.
    controls.stop();
    let outcome = driver::run(
        &mut sim.transport(),
        &sim,
        &job(&encoded),
        &controls,
        |_| {},
    );
    assert!(
        matches!(outcome, Outcome::Stopped { passes_sent: 0, .. }),
        "{outcome:?}"
    );
    assert!(sim.with_machine(|m| m.trace().is_empty()));
    assert!(sim.snapshot().now_s - start < driver::SETTLE_S);
}

#[test]
fn a_cutter_slower_than_estimated_with_a_full_buffer_still_completes() {
    // The 2026-10-18 real cut stopped halfway with a transfer timeout: the machine ran behind
    // the estimates until its buffer filled, and status could not get in.
    // Enough detail to fill the cutter's 4096-byte buffer many times over.
    let mut project = common::project();
    let template = project.prepared.pieces[1].clone();
    for i in 0..120 {
        let mut piece = template.clone();
        piece.id = format!("piece/many{i}");
        let (x, y) = (
            60.0 + f64::from(i % 12) * 18.0,
            60.0 + f64::from(i / 12) * 18.0,
        );
        piece.outer = common::circle(x, y, 6.0);
        project.prepared.pieces.push(piece);
    }
    let plan = plan_sheet(&project, &project.sheets[0].id).unwrap();
    let encoded = gpgl::encode(&plan, &CAMEO_1).unwrap();
    assert!(encoded.bytes.len() > 20_000, "{}", encoded.bytes.len());
    let sim = simulator("slow-full-buffer", encoded.bytes.len());
    let controls = Controls::default();
    let mut pace: f64 = 1.0;
    let outcome = driver::run(&mut sim.transport(), &sim, &job(&encoded), &controls, |p| {
        pace = pace.max(p.pace);
    });
    assert!(matches!(outcome, Outcome::Completed { .. }), "{outcome:?}");
    assert!(pace > 1.5, "pacing learned the machine is slower: {pace}");
    let report = sim.with_machine(|m| parity::check(&plan, m, CAMEO_1.quantization_budget_mm()));
    assert!(report.matches, "{report:#?}");
}

#[test]
fn the_estimated_blade_follows_the_real_one_and_never_goes_back() {
    let (_, encoded) = encoded();
    let sim = simulator("normal", encoded.bytes.len());
    let controls = Controls::default();
    let mut done = 0;
    let mut samples = 0;
    let mut close = 0;
    let mut queued_seen = false;
    let outcome = driver::run(&mut sim.transport(), &sim, &job(&encoded), &controls, |p| {
        let estimate = p.estimate.as_ref().expect("an estimate");
        assert!(estimate.done_bytes >= done, "the estimate never goes back");
        assert!(
            estimate.done_bytes <= p.sent_bytes,
            "nothing unsent is done"
        );
        done = estimate.done_bytes;
        queued_seen |= estimate.ahead.len() > 1;
        // Against the simulator's real blade (it moves at the estimated speed here).
        let real = sim.snapshot().position_mm;
        let at = [
            f64::from(estimate.at[0]) / CAMEO_1.units_per_mm,
            f64::from(estimate.at[1]) / CAMEO_1.units_per_mm,
        ];
        samples += 1;
        if (at[0] - real[0]).hypot(at[1] - real[1]) < 5.0 {
            close += 1;
        }
    });
    assert!(matches!(outcome, Outcome::Completed { .. }), "{outcome:?}");
    assert!(queued_seen, "the queued stretch is reported while sending");
    assert!(
        close * 10 >= samples * 8,
        "within 5 mm of the real blade most of the time: {close} of {samples}"
    );
}

#[test]
fn complete_stream_is_refused_before_any_transport_access() {
    let (_, mut e) = encoded();
    for bad in [
        b"FB1,1\x03".as_slice(),
        b"TB72,0,1\x03",
        b"S1234\x03",
        b"UNKNOWN\x03",
        b"FX34\x03",
        b"D0,6081\x03",
        b"M0,0",
        b"\x1b",
    ] {
        let original = e.bytes.clone();
        e.bytes.extend(bad);
        let sim = simulator("normal", e.bytes.len());
        let outcome = driver::run(
            &mut sim.transport(),
            &sim,
            &job(&e),
            &Controls::default(),
            |_| panic!("no progress before validation"),
        );
        assert!(
            matches!(outcome, Outcome::Refused { .. }),
            "{bad:?}: {outcome:?}"
        );
        assert_eq!(
            sim.received(),
            0,
            "even status queries must wait for validation"
        );
        assert_eq!(sim.snapshot().now_s, 0.0);
        e.bytes = original;
    }
}

fn setup_job(track: bool) -> EncodedPlan {
    let (_, mut e) = encoded();
    e.bytes = if track {
        b"FX20\x03!3\x03FY0\x03M1600,800\x03D1600,900\x03M1600,900\x03".to_vec()
    } else {
        b"FX8\x03!3\x03M1600,800\x03D1600,900\x03M1600,900\x03".to_vec()
    };
    e.passes.clear();
    e.byte_count = e.bytes.len();
    e
}

#[test]
fn long_ready_setup_is_counted_and_never_mistaken_for_completion_or_catch_up() {
    for track in [true, false] {
        let e = setup_job(track);
        let mut profile = Profile::cameo_1();
        let delay = 6.0; // deliberately longer than READY_HOLD_S
        if track {
            profile.track_enhancing_s = delay;
        } else {
            profile.alignment_tick_s = delay;
        }
        let sim = Simulator::new(profile, Clock::Virtual, &Scenario::default());
        let setup = driver::SetupTiming {
            before_motion_s: delay,
        };
        let job = job(&e);
        assert!(
            (driver::estimate_with_setup_s(&job, setup) - driver::estimate_s(&job) - delay).abs()
                < 1e-9
        );
        let mut fractions = Vec::new();
        let mut ready_during_setup = false;
        let result = driver::run_experiment(
            &mut sim.transport(),
            &sim,
            &job,
            &Controls::default(),
            track.then_some(gpgl::Experiment::Track),
            setup,
            |p| {
                let now = sim.snapshot().now_s;
                if sim.with_machine(|m| m.busy())
                    && sim.snapshot().status == cutcraft_sim::Status::Ready
                {
                    ready_during_setup = true;
                    assert!(
                        p.remaining_s > 0.0,
                        "setup must not be retired by Ready catch-up at {now}"
                    );
                    assert!(!p.estimate.as_ref().unwrap().down);
                }
                fractions.push(p.fraction);
            },
        );
        assert!(matches!(result, Outcome::Completed { .. }), "{result:?}");
        assert!(ready_during_setup);
        assert!(fractions.iter().all(|f| *f < 1.0));
        sim.with_machine(|m| {
            assert!(!m.busy(), "completion before queued work finished");
            assert!(m.faults().is_empty(), "{:?}", m.faults());
            let cut = m.trace().iter().find(|s| s.tool_down).unwrap();
            assert_eq!(cut.to, [45.0, 80.0]);
            assert!(cut.start_s >= driver::SETTLE_S + delay);
        });
    }
}

#[test]
fn stop_during_ready_setup_drains_without_sending_a_draw() {
    for delay in [8.0, 8.25, 8.5] {
        let e = setup_job(true);
        let mut profile = Profile::cameo_1();
        profile.track_enhancing_s = delay;
        let sim = Simulator::new(profile, Clock::Virtual, &Scenario::default());
        let controls = Controls::default();
        let mut requested = None;
        let outcome = driver::run_experiment(
            &mut sim.transport(),
            &sim,
            &job(&e),
            &controls,
            Some(gpgl::Experiment::Track),
            driver::SetupTiming {
                before_motion_s: delay,
            },
            |p| {
                let now = sim.snapshot().now_s;
                if requested.is_none() && now >= driver::SETTLE_S + 3.0 && p.phase == Phase::Sending
                {
                    requested = Some(now);
                    controls.stop();
                }
            },
        );
        assert!(requested.is_some());
        assert!(
            matches!(outcome, Outcome::Stopped { idle: true, .. }),
            "{outcome:?}"
        );
        sim.with_machine(|m| {
            assert!(!m.busy(), "Stop must not report idle during deferred setup");
            assert!(
                m.trace().iter().all(|s| !s.tool_down),
                "no drawing may be queued during setup"
            );
            assert!(m.faults().is_empty());
        });
    }
}

#[test]
fn built_in_materials_and_calibration_plans_validate() {
    for material in cutcraft_job::Material::defaults() {
        for plan in [
            cutcraft_path::calibration_plan(&material, &cutcraft_job::MachineProfile::cameo_1())
                .unwrap(),
            cutcraft_path::letter_calibration_plan(
                &material,
                &cutcraft_job::MachineProfile::cameo_1(),
            )
            .unwrap(),
        ] {
            let e = gpgl::encode(&plan, &CAMEO_1).unwrap();
            gpgl::validate(&e.bytes, &CAMEO_1, e.cutting_area_mm).unwrap();
        }
    }
}

#[test]
fn orientation_hold_cannot_be_bypassed_by_experimental_or_hand_built_jobs() {
    for (setting, experiment) in [
        (b"FN0\x03".as_slice(), gpgl::Experiment::Orientation),
        (b"TB50,0\x03".as_slice(), gpgl::Experiment::Portrait),
    ] {
        let (_, mut e) = encoded();
        e.bytes.extend(setting); // valid prefix must not be sent either
        let sim = simulator("normal", e.bytes.len());
        let outcome = driver::run_experiment(
            &mut sim.transport(),
            &sim,
            &job(&e),
            &Controls::default(),
            Some(experiment),
            driver::SetupTiming::default(),
            |_| panic!("must refuse before progress"),
        );
        assert!(
            matches!(outcome,Outcome::Refused {ref reason} if reason.contains("suspended")),
            "{outcome:?}"
        );
        assert_eq!(
            sim.received(),
            0,
            "even the valid prefix and status queries must not be sent"
        );
    }
}

#[test]
fn staged_frame_capability_never_accepts_motion_or_extra_bytes() {
    let (_, base) = encoded();
    for command in [driver::FrameCommand::Fn0, driver::FrameCommand::Tb50] {
        for extra in [
            b"M0,0\x03".as_slice(),
            b"D1400,1200\x03",
            b"FN0\x03",
            b"TB50,0\x03",
        ] {
            for before in [true, false] {
                let mut e = base.clone();
                e.bytes = if before {
                    [extra, command.bytes()].concat()
                } else {
                    [command.bytes(), extra].concat()
                };
                let sim = simulator("normal", e.bytes.len());
                let outcome = driver::run_frame_step(
                    &mut sim.transport(),
                    &sim,
                    &job(&base),
                    &Controls::default(),
                    command,
                    |_| {},
                );
                // A normal full job is also rejected under the frame-only permission.
                assert!(matches!(outcome, Outcome::Refused { .. }));
                let frame_job = Job {
                    plan: &e,
                    ..job(&base)
                };
                let outcome = driver::run_frame_step(
                    &mut sim.transport(),
                    &sim,
                    &frame_job,
                    &Controls::default(),
                    command,
                    |_| {},
                );
                assert!(matches!(outcome, Outcome::Refused { .. }), "{outcome:?}");
                assert_eq!(sim.received(), 0);
            }
        }
    }
}
