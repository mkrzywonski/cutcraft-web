//! Incremental parser for the GP-GL bytes a driver sends.
//!
//! Framing follows the working hypothesis in spec §7.3: text commands end with ETX (0x03);
//! the two-byte escape sequences ESC EOT (initialise) and ESC ENQ (status) are not framed and are
//! handled as soon as they arrive. Coordinates are device units with the feed axis (Y) first.
//! Anything outside the model's command set is an error, never silently ignored.

use serde::Serialize;

/// End of a text command.
pub const ETX: u8 = 0x03;
/// Starts a two-byte escape sequence.
pub const ESC: u8 = 0x1b;
/// Longest text command accepted before the parser gives up on it.
pub const MAX_COMMAND_LEN: usize = 4096;

/// A point in device units: `y` along the feed, `x` across the mat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Point {
    pub y: i32,
    pub x: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Command {
    /// ESC EOT.
    Initialize,
    /// ESC ENQ.
    Status,
    /// `FG`: report the firmware version.
    Firmware,
    /// `M y,x[,y,x…]`: move with the tool up.
    Move(Vec<Point>),
    /// `D y,x[,y,x…]`: move with the tool down.
    Draw(Vec<Point>),
    /// `FX n[,…]`.
    Force(i32),
    /// `!n[,…]`.
    Speed(i32),
    /// `FC…`: blade offset settings, kept verbatim.
    BladeOffset(Vec<i32>),
    /// `FW…`: media type, kept verbatim.
    Media(Vec<i32>),
    /// Candidate setup commands, retained without claiming verified physical behaviour.
    Setup { code: String, args: Vec<i32> },
    /// `FF…` / `FE…`: corner settings, kept verbatim.
    Corner { code: String, args: Vec<i32> },
}

impl Command {
    /// Escape sequences are acted on at once rather than queued behind motion.
    pub fn is_immediate(&self) -> bool {
        matches!(self, Self::Initialize | Self::Status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ParseError {
    UnknownEscape(u8),
    UnknownCommand(String),
    BadArguments { command: String, detail: String },
    NotAscii(u8),
    TooLong,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEscape(byte) => write!(f, "unknown escape sequence ESC 0x{byte:02x}"),
            Self::UnknownCommand(text) => write!(f, "unknown command {text:?}"),
            Self::BadArguments { command, detail } => write!(f, "{command}: {detail}"),
            Self::NotAscii(byte) => write!(f, "unexpected byte 0x{byte:02x} in a command"),
            Self::TooLong => write!(f, "command longer than {MAX_COMMAND_LEN} bytes"),
        }
    }
}

/// What one input byte completed, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// A command and the number of bytes it occupied (including ETX, or both escape bytes).
    Command(Command, usize),
    /// A rejected command and the bytes it occupied.
    Error(ParseError, usize),
}

#[derive(Debug, Default)]
pub struct Parser {
    pending: Vec<u8>,
    escape: bool,
    overflowed: bool,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes of an unfinished text command.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Whether the next byte completes an escape sequence.
    pub fn in_escape(&self) -> bool {
        self.escape
    }

    pub fn push(&mut self, byte: u8) -> Option<Parsed> {
        if self.escape {
            self.escape = false;
            return Some(match byte {
                0x04 => Parsed::Command(Command::Initialize, 2),
                0x05 => Parsed::Command(Command::Status, 2),
                other => Parsed::Error(ParseError::UnknownEscape(other), 2),
            });
        }
        match byte {
            // Escape sequences may arrive between the bytes of a text command (hypothesis).
            ESC => {
                self.escape = true;
                None
            }
            ETX => {
                let len = self.pending.len() + 1;
                let text = std::mem::take(&mut self.pending);
                if std::mem::take(&mut self.overflowed) {
                    return Some(Parsed::Error(ParseError::TooLong, len));
                }
                // Only printable ASCII is ever stored.
                let text = String::from_utf8_lossy(&text);
                Some(match parse_text(&text) {
                    Ok(command) => Parsed::Command(command, len),
                    Err(error) => Parsed::Error(error, len),
                })
            }
            0x20..=0x7e => {
                if self.pending.len() >= MAX_COMMAND_LEN {
                    self.overflowed = true;
                } else {
                    self.pending.push(byte);
                }
                None
            }
            other => Some(Parsed::Error(ParseError::NotAscii(other), 1)),
        }
    }
}

fn parse_text(text: &str) -> Result<Command, ParseError> {
    let (code, rest) = split_code(text)?;
    let args = parse_ints(code, rest)?;
    let bad = |detail: &str| ParseError::BadArguments {
        command: code.to_owned(),
        detail: detail.to_owned(),
    };
    match code {
        "FG" if args.is_empty() => Ok(Command::Firmware),
        "FG" => Err(bad("takes no arguments")),
        "FN" | "TB" | "\\" | "Z" | "L" | "FY" | "FU" | "H" | "SO" => {
            let valid = match code {
                "FN" | "L" | "SO" => args == [0],
                "TB" => args == [50, 0],
                "FY" => matches!(args.as_slice(), [0 | 1]),
                "FU" => matches!(args.as_slice(), [1..=60000]),
                "\\" | "Z" => args.len() == 2 && args.iter().all(|n| *n >= 0),
                "H" => args.is_empty(),
                _ => false,
            };
            if valid {
                Ok(Command::Setup {
                    code: code.to_owned(),
                    args,
                })
            } else {
                Err(bad("unsupported candidate arguments"))
            }
        }
        "M" | "D" => {
            if args.is_empty() || args.len() % 2 != 0 {
                return Err(bad("needs one or more y,x pairs"));
            }
            let points = args
                .chunks_exact(2)
                .map(|pair| Point {
                    y: pair[0],
                    x: pair[1],
                })
                .collect();
            Ok(if code == "M" {
                Command::Move(points)
            } else {
                Command::Draw(points)
            })
        }
        "FX" | "!" => match args.first() {
            Some(&value) if args.len() <= 2 => Ok(if code == "FX" {
                Command::Force(value)
            } else {
                Command::Speed(value)
            }),
            _ => Err(bad("needs a value")),
        },
        "FC" if !args.is_empty() => Ok(Command::BladeOffset(args)),
        "FW" if !args.is_empty() => Ok(Command::Media(args)),
        "FF" | "FE" if !args.is_empty() => Ok(Command::Corner {
            code: code.to_owned(),
            args,
        }),
        _ => Err(bad("needs arguments")),
    }
}

fn split_code(text: &str) -> Result<(&str, &str), ParseError> {
    for code in [
        "TB", "FN", "FY", "FU", "SO", "FG", "FX", "FC", "FW", "FF", "FE", "M", "D", "!", "\\", "Z",
        "L", "H",
    ] {
        if let Some(rest) = text.strip_prefix(code) {
            return Ok((code, rest));
        }
    }
    Err(ParseError::UnknownCommand(text.to_owned()))
}

fn parse_ints(code: &str, rest: &str) -> Result<Vec<i32>, ParseError> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    rest.split(',')
        .map(|field| {
            field
                .trim()
                .parse::<i32>()
                .map_err(|_| ParseError::BadArguments {
                    command: code.to_owned(),
                    detail: format!("{field:?} is not an integer"),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(bytes: &[u8]) -> Vec<Parsed> {
        let mut parser = Parser::new();
        bytes.iter().filter_map(|&b| parser.push(b)).collect()
    }

    #[test]
    fn frames_text_and_escape_commands() {
        let parsed = parse_all(b"\x1b\x04FX10\x03!5,0\x03M 100,200\x03D10,20,30,40\x03\x1b\x05");
        assert_eq!(
            parsed,
            vec![
                Parsed::Command(Command::Initialize, 2),
                Parsed::Command(Command::Force(10), 5),
                Parsed::Command(Command::Speed(5), 5),
                Parsed::Command(Command::Move(vec![Point { y: 100, x: 200 }]), 10),
                Parsed::Command(
                    Command::Draw(vec![Point { y: 10, x: 20 }, Point { y: 30, x: 40 }]),
                    13
                ),
                Parsed::Command(Command::Status, 2),
            ]
        );
    }

    #[test]
    fn escape_inside_a_text_command_is_handled_at_once() {
        let parsed = parse_all(b"M1\x1b\x05,2\x03");
        assert_eq!(
            parsed,
            vec![
                Parsed::Command(Command::Status, 2),
                Parsed::Command(Command::Move(vec![Point { y: 1, x: 2 }]), 5),
            ]
        );
    }

    #[test]
    fn rejects_unknown_and_malformed_commands() {
        let parsed = parse_all(b"TB50\x03M1\x03Dx,1\x03FG1\x03\x1b\x07\x00");
        assert!(matches!(
            &parsed[0],
            Parsed::Error(ParseError::BadArguments { .. }, 5)
        ));
        assert!(matches!(
            &parsed[1],
            Parsed::Error(ParseError::BadArguments { .. }, 3)
        ));
        assert!(matches!(
            &parsed[2],
            Parsed::Error(ParseError::BadArguments { .. }, 5)
        ));
        assert!(matches!(
            &parsed[3],
            Parsed::Error(ParseError::BadArguments { .. }, 4)
        ));
        assert_eq!(parsed[4], Parsed::Error(ParseError::UnknownEscape(0x07), 2));
        assert_eq!(parsed[5], Parsed::Error(ParseError::NotAscii(0), 1));
    }

    #[test]
    fn overlong_command_is_rejected_whole() {
        let mut bytes = vec![b'M'; MAX_COMMAND_LEN + 10];
        bytes.push(ETX);
        let parsed = parse_all(&bytes);
        assert_eq!(
            parsed,
            vec![Parsed::Error(ParseError::TooLong, MAX_COMMAND_LEN + 1)]
        );
    }
}
