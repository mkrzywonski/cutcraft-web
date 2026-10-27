use std::time::Duration;

use cutcraft_device::transport::{Transport, TransportError};
use cutcraft_sim::harness::{self, query_status, read_reply};
use cutcraft_sim::{
    Clock, FaultKind, Media, Profile, Report, Scenario, Simulator, Status, SvgOptions, mat,
};

const TEST_CUT: &[u8] = include_bytes!("fixtures/test-cut.gpgl");
const TIMEOUT: Duration = Duration::from_secs(1);

fn simulator(scenario: &str) -> Simulator {
    let scenario = Scenario::named(scenario, TEST_CUT.len()).expect("known scenario");
    Simulator::new(Profile::cameo_1(), Clock::Virtual, &scenario)
}

fn report(sim: &Simulator) -> Report {
    sim.with_machine(Report::of)
}

#[test]
fn test_cut_runs_clean_and_matches_the_authored_geometry() {
    let sim = simulator("normal");
    let mut transport = sim.transport();
    let outcome = harness::send(&mut transport, TEST_CUT, 64, TIMEOUT);
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.confirmed, TEST_CUT.len());
    let status = harness::wait_until_stopped(
        &mut transport,
        Duration::from_millis(250),
        Duration::from_secs(120),
        |d| sim.advance(d),
    );
    assert_eq!(status, Ok(Status::Ready));

    let report = report(&sim);
    assert!(report.faults.is_empty(), "{:?}", report.faults);
    assert_eq!(report.cut_segments, 7);
    assert_eq!(report.repeated_cut_segments, 0);
    // Square perimeter plus the triangle's three sides.
    let triangle = 15.4 + 2.0 * 7.7_f64.hypot(15.4);
    assert!((report.cut_mm - (4.0 * 25.4 + triangle)).abs() < 0.01);
    // Speed 3 is 30 mm/s: the cutting alone takes over 4 s.
    assert!(report.duration_s > 4.0 && report.duration_s < 20.0);

    let svg = sim.with_machine(|m| mat::svg(m, SvgOptions { travel: true }));
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("M20.000 20.000L45.400 20.000"));
}

#[test]
fn firmware_and_status_queries_answer_without_motion() {
    let sim = simulator("normal");
    let mut transport = sim.transport();
    assert_eq!(query_status(&mut transport, TIMEOUT), Ok(Status::Ready));
    transport.write(b"FG\x03", TIMEOUT).expect("write FG");
    assert_eq!(
        read_reply(&mut transport, TIMEOUT).as_deref(),
        Ok(&b"CAMEO V1.20"[..])
    );
    assert!(sim.with_machine(|m| m.trace().is_empty()));
}

#[test]
fn no_media_is_reported_and_motion_is_refused() {
    let sim = simulator("no-media");
    let mut transport = sim.transport();
    assert_eq!(query_status(&mut transport, TIMEOUT), Ok(Status::NoMedia));
    harness::send(&mut transport, TEST_CUT, 64, TIMEOUT);
    sim.run_until_idle(Duration::from_secs(60));
    let report = report(&sim);
    assert_eq!(report.cut_segments, 0);
    assert!(report.faults.iter().all(|f| f.kind == FaultKind::NoMedia));
    assert!(!report.faults.is_empty());
}

#[test]
fn disconnect_keeps_running_the_buffer_and_old_handles_stay_dead() {
    let sim = simulator("disconnect-mid-cut");
    let mut transport = sim.transport();
    let outcome = harness::send(&mut transport, TEST_CUT, 16, TIMEOUT);
    assert_eq!(outcome.error.as_deref(), Some("the cutter is disconnected"));
    assert_eq!(sim.received(), TEST_CUT.len() / 2);
    assert!(sim.run_until_idle(Duration::from_secs(60)));
    assert!(report(&sim).cut_segments > 0, "buffered motion still ran");

    sim.reconnect();
    assert_eq!(
        query_status(&mut transport, TIMEOUT),
        Err(TransportError::Disconnected)
    );
    let mut fresh = sim.transport();
    assert_eq!(query_status(&mut fresh, TIMEOUT), Ok(Status::Ready));
}

#[test]
fn power_off_loses_buffered_motion() {
    let sim = simulator("power-off-mid-cut");
    let mut transport = sim.transport();
    let outcome = harness::send(&mut transport, TEST_CUT, TEST_CUT.len(), TIMEOUT);
    assert!(outcome.error.is_some());
    let report = report(&sim);
    assert!(report.faults.iter().any(|f| f.kind == FaultKind::PowerLost));
    assert!(sim.run_until_idle(Duration::from_secs(1)));
    assert_eq!(sim.snapshot().buffered_bytes, 0);
    sim.reconnect();
    assert_eq!(
        query_status(&mut sim.transport(), TIMEOUT),
        Err(TransportError::Disconnected),
        "a powered-off cutter cannot reconnect"
    );
    sim.power_on();
    sim.reconnect();
    assert_eq!(
        query_status(&mut sim.transport(), TIMEOUT),
        Ok(Status::Ready)
    );
}

#[test]
fn timeout_after_delivery_leaves_the_bytes_delivered() {
    let sim = simulator("timeout-after-delivery");
    let mut transport = sim.transport();
    let outcome = harness::send(&mut transport, TEST_CUT, TEST_CUT.len(), TIMEOUT);
    assert_eq!(
        outcome.error.as_deref(),
        Some("transfer timed out; delivery is uncertain")
    );
    assert_eq!(outcome.confirmed, 0);
    assert_eq!(sim.received(), TEST_CUT.len() / 2);
}

#[test]
fn stall_holds_until_cleared() {
    let sim = simulator("stall");
    let mut transport = sim.transport();
    let first = harness::send(&mut transport, TEST_CUT, 8, TIMEOUT);
    assert_eq!(first.error.as_deref(), Some("the endpoint is stalled"));
    assert_eq!(
        transport.write(&TEST_CUT[first.confirmed..], TIMEOUT),
        Err(TransportError::Stalled)
    );
    transport.clear_halt().expect("clear halt");
    let rest = harness::send(&mut transport, &TEST_CUT[first.confirmed..], 8, TIMEOUT);
    assert_eq!(rest.error, None);
    assert!(sim.run_until_idle(Duration::from_secs(60)));
    assert!(report(&sim).faults.is_empty());
}

#[test]
fn unload_mid_cut_refuses_the_remaining_motion() {
    let sim = simulator("unload-mid-cut");
    let mut transport = sim.transport();
    harness::send(&mut transport, TEST_CUT, 8, TIMEOUT);
    sim.run_until_idle(Duration::from_secs(60));
    let report = report(&sim);
    assert!(report.faults.iter().any(|f| f.kind == FaultKind::NoMedia));
    assert_eq!(report.media, None);
}

#[test]
fn slow_status_times_out_a_short_read() {
    let sim = simulator("slow-status");
    let mut transport = sim.transport();
    transport.write(b"\x1b\x05", TIMEOUT).expect("write");
    let mut byte = [0u8; 1];
    assert_eq!(
        transport.read(&mut byte, Duration::from_millis(100)),
        Err(TransportError::Timeout)
    );
    assert_eq!(transport.read(&mut byte, TIMEOUT), Ok(1));
    assert_eq!(byte[0], b'0');
}

#[test]
fn full_buffer_blocks_writes_until_motion_frees_space() {
    let sim = simulator("normal");
    let mut transport = sim.transport();
    let mut long = Vec::new();
    for i in 0..1500 {
        long.extend(format!("D{},{}\x03", 400 + (i % 2) * 400, 400).bytes());
    }
    let outcome = harness::send(&mut transport, &long, 512, Duration::from_millis(50));
    assert_eq!(
        outcome.error.as_deref(),
        Some("transfer timed out; delivery is uncertain")
    );
    let buffer = Profile::cameo_1().buffer_bytes;
    assert!(sim.snapshot().buffered_bytes <= buffer);
    // Retracing the same line is counted, not hidden.
    assert!(sim.run_until_idle(Duration::from_secs(600)));
    assert!(report(&sim).repeated_cut_segments > 0);
}

#[test]
fn realtime_clock_follows_the_wall_clock() {
    let scenario = Scenario::default();
    let sim = Simulator::new(Profile::cameo_1(), Clock::Realtime, &scenario);
    let mut transport = sim.transport();
    transport
        .write(b"!10\x03D0,40\x03", TIMEOUT)
        .expect("write");
    // Idle until now, it starts a moment later, answering Ready meanwhile.
    std::thread::sleep(Duration::from_millis(500));
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.status, Status::Ready);
    assert_eq!(snapshot.position_mm, [0.0, 0.0]);
    std::thread::sleep(Duration::from_millis(700));
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.status, Status::Ready);
    assert_eq!(snapshot.position_mm, [2.0, 0.0]);
    assert_eq!(snapshot.media, Some(Media::Mat12x12));
}

#[test]
fn candidate_setup_is_retained_and_unknown_commands_still_fault() {
    let mut m = cutcraft_sim::Machine::new(cutcraft_sim::Profile::cameo_1());
    m.load(cutcraft_sim::Media::Mat12x12);
    let data=b"FN0\x03TB50,0\x03\\0,0\x03Z5000,4000\x03L0\x03FY1\x03FU3000\x03H\x03SO0\x03FC18\x03FE0,0\x03FF0,0,0\x03";
    assert_eq!(m.accept(data), data.len());
    m.advance(5.0);
    assert!(m.faults().is_empty(), "{:?}", m.faults());
    assert_eq!(m.setup_settings().get("FY").unwrap(), &[1]);
    assert_eq!(m.setup_settings().get("Z").unwrap(), &[5000, 4000]);
    m.accept(b"FB1,1\x03TB72,0,1\x03");
    assert_eq!(m.faults().len(), 2);
}

#[test]
fn track_delay_requires_enabled_track_and_high_force() {
    for (force, fy, delayed) in [(20, 0, true), (20, 1, false), (18, 0, false)] {
        let mut p = cutcraft_sim::Profile::cameo_1();
        p.track_enhancing_s = 7.0;
        let mut m = cutcraft_sim::Machine::new(p);
        m.load(cutcraft_sim::Media::Mat12x12);
        m.accept(format!("FX{force}\x03FY{fy}\x03M1600,800\x03").as_bytes());
        m.advance(3.0);
        if delayed {
            assert_eq!(m.status(), cutcraft_sim::Status::Ready);
            assert!(m.busy());
        }
        m.advance(30.0);
        let first = m.trace().first().unwrap();
        assert_eq!(first.start_s > 5.0, delayed);
    }
}

#[test]
fn fn0_origin_hypothesis_reproduces_the_off_paper_translation() {
    let mut m = cutcraft_sim::Machine::new(cutcraft_sim::Profile::cameo_1());
    m.load(cutcraft_sim::Media::Mat12x12);
    // Original authored reproduction: prior job's endpoint, FN0, then a square whose logical
    // coordinates fit on Letter. Direct simulator input deliberately bypasses the real driver,
    // which now refuses FN0 before sending any bytes.
    m.accept(b"M2450,2650\x03FN0\x03M2800,800\x03D2800,1300,3300,1300,3300,800,2800,800\x03");
    m.advance(60.0);
    assert!(m.faults().is_empty(), "{:?}", m.faults());
    let cuts: Vec<_> = m.trace().iter().filter(|s| s.tool_down).collect();
    assert_eq!(cuts[0].from, [172.5, 262.5]);
    assert_eq!(cuts[0].to, [197.5, 262.5]);
    assert!(
        cuts.iter().any(|s| s.to[1] > 279.4),
        "logical paper bounds did not bound physical motion"
    );
}
