//! What a cutter runs: a plan encoded for the cutter's model, with the speeds the driver's
//! estimates use and a digest computed here from what is actually sent (spec §10).

use cutcraft_device::driver::Job;
use cutcraft_device::gpgl::{self, EncodedPlan, Model};
use cutcraft_job::{Diagnostic, MachineProfile};
use cutcraft_path::CutPlan;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct Runnable {
    pub encoded: EncodedPlan,
    /// Bounds of every blade-down path, `[x0, y0, x1, y1]` mm: the area it takes on the mat.
    pub bounds_mm: Option<[f64; 4]>,
    /// SHA-256 of the exact bytes sent and the timing the driver uses, computed here. The
    /// client's `plan_id` is never trusted for this.
    pub digest: String,
    pub mm_per_s: f64,
    pub travel_mm_s: f64,
    pub tool_change_s: f64,
    pub point_s: f64,
    pub turn_s_per_rad: f64,
    pub units_per_mm: f64,
}

impl Runnable {
    /// Encodes `plan` for `model`. The encoder refuses settings outside the model's ranges and
    /// paths that leave its area; its stream allowlist is the final check.
    pub fn new(
        plan: &CutPlan,
        model: &Model,
        machine: &MachineProfile,
    ) -> Result<Self, Vec<Diagnostic>> {
        let encoded = gpgl::encode(plan, model)?;
        Ok(Self::from_encoded(
            encoded,
            plan.bounds_mm,
            f64::from(plan.material.speed) * machine.speed_step_mm_s,
            machine,
            model.units_per_mm,
        ))
    }

    fn from_encoded(
        encoded: EncodedPlan,
        bounds_mm: Option<[f64; 4]>,
        mm_per_s: f64,
        machine: &MachineProfile,
        units_per_mm: f64,
    ) -> Self {
        let mut hash = Sha256::new();
        hash.update(encoded.model.as_bytes());
        hash.update(&encoded.bytes);
        for value in [
            mm_per_s,
            machine.travel_mm_s,
            machine.tool_change_s,
            machine.point_s,
            machine.turn_s_per_rad,
            units_per_mm,
        ] {
            hash.update(value.to_le_bytes());
        }
        let digest = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Self {
            encoded,
            bounds_mm,
            digest,
            mm_per_s,
            travel_mm_s: machine.travel_mm_s,
            tool_change_s: machine.tool_change_s,
            point_s: machine.point_s,
            turn_s_per_rad: machine.turn_s_per_rad,
            units_per_mm,
        }
    }

    /// The driver's view of it.
    pub fn job(&self) -> Job<'_> {
        Job {
            plan: &self.encoded,
            mm_per_s: self.mm_per_s,
            travel_mm_s: self.travel_mm_s,
            tool_change_s: self.tool_change_s,
            point_s: self.point_s,
            turn_s_per_rad: self.turn_s_per_rad,
            units_per_mm: self.units_per_mm,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use cutcraft_device::gpgl::CAMEO_1;
    use cutcraft_job::{Material, OffsetMethod, Status};

    use super::*;

    pub(crate) fn pen() -> Material {
        Material {
            name: "Pen (test)".into(),
            blade: 1,
            force: 10,
            speed: 5,
            passes: 1,
            mirror: false,
            status: Status::Provisional,
            offset_method: OffsetMethod::Off,
            offset_mm: 0.0,
            overcut_mm: 0.0,
            notes: String::new(),
            roll: false,
        }
    }

    /// The calibration sheet, encoded for the original Cameo.
    pub(crate) fn calibration() -> Runnable {
        let machine = MachineProfile::cameo_1();
        let plan = cutcraft_path::calibration_plan(&pen(), &machine).unwrap();
        Runnable::new(&plan, &CAMEO_1, &machine).unwrap()
    }

    /// The calibration sheet, claiming a 10 mm square at `at` as its area (state-machine tests
    /// only care about the area).
    pub(crate) fn runnable_at(at: f64) -> Arc<Runnable> {
        let mut run = calibration();
        run.bounds_mm = Some([at, at, at + 10.0, at + 10.0]);
        Arc::new(run)
    }

    #[test]
    fn the_digest_follows_the_bytes_and_the_timing() {
        let machine = MachineProfile::cameo_1();
        let plan = cutcraft_path::calibration_plan(&pen(), &machine).unwrap();
        let a = Runnable::new(&plan, &CAMEO_1, &machine).unwrap();
        let b = Runnable::new(&plan, &CAMEO_1, &machine).unwrap();
        assert_eq!(a.digest, b.digest);
        let mut faster = pen();
        faster.speed = 6;
        let plan = cutcraft_path::calibration_plan(&faster, &machine).unwrap();
        let c = Runnable::new(&plan, &CAMEO_1, &machine).unwrap();
        assert_ne!(a.digest, c.digest);
    }
}
