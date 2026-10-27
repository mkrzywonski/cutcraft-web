//! Whole-stream validation. Independently authored from the protocol facts in spec §7.3.
//! Candidate commands require a typed experiment permission; ordinary cuts never acquire it.
use super::Model;
use serde::Serialize;

const MAX_COMMAND: usize = 4096;
/// An unverified setting permitted only by the hardware probe entry point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Experiment {
    Corners,
    Sharpening,
    Track,
    Orientation,
    Portrait,
    LineType,
    Area,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StreamError {
    pub byte: usize,
    pub reason: String,
}
impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Job refused at byte {}: {}. Nothing sent.",
            self.byte, self.reason
        )
    }
}
impl std::error::Error for StreamError {}

pub fn validate(bytes: &[u8], model: &Model, area_mm: [f64; 2]) -> Result<(), StreamError> {
    check(bytes, model, area_mm, None)
}

pub fn validate_experiment(
    bytes: &[u8],
    model: &Model,
    area_mm: [f64; 2],
    experiment: Experiment,
) -> Result<(), StreamError> {
    check(bytes, model, area_mm, Some(experiment))
}

fn check(
    bytes: &[u8],
    model: &Model,
    area: [f64; 2],
    experiment: Option<Experiment>,
) -> Result<(), StreamError> {
    let fail = |byte, reason: &str| StreamError {
        byte,
        reason: reason.into(),
    };
    if !area.iter().all(|v| v.is_finite() && *v > 0.0) || area[1] > model.max_length_mm {
        return Err(fail(0, "invalid model cutting area"));
    }
    let limit = [
        (area[0].min(model.width_mm) * model.units_per_mm).floor() as i32,
        (area[1] * model.units_per_mm).floor() as i32,
    ];
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i] == 0x1b {
            if !matches!(bytes.get(i + 1), Some(0x04 | 0x05)) {
                return Err(fail(i, "unsupported or incomplete escape"));
            }
            i += 2;
            continue;
        }
        let tail = &bytes[i..];
        let end = tail
            .iter()
            .position(|b| *b == 3)
            .ok_or_else(|| fail(start, "missing ETX"))?;
        if end == 0 || end > MAX_COMMAND || !tail[..end].iter().all(|b| (0x20..=0x7e).contains(b)) {
            return Err(fail(start, "empty, overlong or non-ASCII command"));
        }
        let text =
            std::str::from_utf8(&tail[..end]).map_err(|_| fail(start, "non-ASCII command"))?;
        let code = [
            "TB", "FX", "FC", "FE", "FF", "FY", "FN", "FG", "M", "D", "!", "L", "\\", "Z",
        ]
        .into_iter()
        .find(|code| text.starts_with(code))
        .ok_or_else(|| fail(start, "unsupported command"))?;
        // The 2026-10-23 physical probe translated drawings off the sheet after FN0.
        // TB50 followed the changed frame and remains unisolated. Experimental permission
        // must not bypass this hold, including for a hand-built stream with a valid prefix.
        if matches!(code, "FN" | "TB") {
            return Err(fail(
                start,
                "orientation commands are suspended after an off-paper probe; the physical coordinate frame is unverified",
            ));
        }
        let rest = &text[code.len()..];
        let args: Vec<i32> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split(',')
                .map(|s| {
                    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(fail(start, "arguments must be unsigned decimal integers"));
                    }
                    s.parse().map_err(|_| fail(start, "argument overflow"))
                })
                .collect::<Result<_, _>>()?
        };
        let pair_inside = |a: &[i32]| a.len() == 2 && a[0] <= limit[1] && a[1] <= limit[0];
        let valid = match code {
            "M" | "D" => {
                !args.is_empty()
                    && args.len().is_multiple_of(2)
                    && args.chunks_exact(2).all(pair_inside)
            }
            "FX" => {
                args.len() == 1
                    && (i32::from(model.force_range.0)..=i32::from(model.force_range.1))
                        .contains(&args[0])
            }
            "!" => {
                args.len() == 1
                    && (i32::from(model.speed_range.0)..=i32::from(model.speed_range.1))
                        .contains(&args[0])
            }
            // Existing machine compensation is still unverified. This is a conservative probe
            // ceiling (10 mm at the current conversion), not a measured firmware maximum.
            "FC" => args.len() == 1 && args[0] <= model.max_offset_units,
            "FG" => args.is_empty(),
            "FE" => {
                experiment == Some(Experiment::Corners) && matches!(args.as_slice(), [0 | 1, 0])
            }
            "FF" => experiment == Some(Experiment::Sharpening) && args == [0, 0, 0],
            "FY" => experiment == Some(Experiment::Track) && matches!(args.as_slice(), [0 | 1]),
            "L" => experiment == Some(Experiment::LineType) && args == [0],
            "\\" | "Z" => experiment == Some(Experiment::Area) && pair_inside(&args),
            _ => false,
        };
        if !valid {
            return Err(fail(
                start,
                "unsupported argument form, range or experiment permission",
            ));
        }
        i += end + 1;
    }
    if bytes.is_empty() {
        return Err(fail(0, "empty job"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpgl::CAMEO_1;
    #[test]
    fn framing_ranges_and_permissions_are_checked_for_the_complete_stream() {
        let area = [304.0, 304.8];
        assert!(
            validate(
                b"FX10\x03!5\x03FC0\x03M1200,300\x03D1400,500\x03",
                &CAMEO_1,
                area
            )
            .is_ok()
        );
        assert!(validate(b"\x1b\x04FG\x03\x1b\x05", &CAMEO_1, area).is_ok());
        for bad in [
            b"FB1,1\x03".as_slice(),
            b"TB72,0,1\x03",
            b"S1234\x03",
            b"FX34\x03",
            b"!0\x03",
            b"FC201\x03",
            b"D0,6081\x03",
            b"M1\x03",
            b"FG1\x03",
            b"FY1\x03",
            b"\x1b",
            b"\x1b\x07",
            b"M1\x1b\x05,2\x03",
            b"\x03",
            b"M0,0",
            b"M-1,0\x03",
            b"M0,0\x80\x03",
            b"FX1,1\x03",
        ] {
            let mut stream = b"FX10\x03M0,0\x03".to_vec();
            stream.extend(bad);
            assert!(validate(&stream, &CAMEO_1, area).is_err(), "{bad:?}");
        }
        let mut long = vec![b'0'; MAX_COMMAND + 1];
        long.push(3);
        assert!(validate(&long, &CAMEO_1, area).is_err());
        assert!(validate_experiment(b"FY0\x03", &CAMEO_1, area, Experiment::Track).is_ok());
        assert!(
            validate_experiment(b"FY0\x03FE0,0\x03", &CAMEO_1, area, Experiment::Track).is_err()
        );
        assert!(
            validate_experiment(b"TB72,0,1\x03", &CAMEO_1, area, Experiment::Portrait).is_err()
        );
    }
}
