use std::{
    io::{self, Read, Write},
    time::{Duration, Instant},
};

use sdrmm_wire::NanoVnaComplex;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const PROMPT: &[u8] = b"ch>";

pub trait Connection: Read + Write + Send {}

impl<T: Read + Write + Send> Connection for T {}

pub struct Session<'a> {
    connection: &'a mut dyn Connection,
    pending: Vec<u8>,
}

impl<'a> Session<'a> {
    pub fn new(connection: &'a mut dyn Connection) -> Self {
        Self {
            connection,
            pending: Vec::new(),
        }
    }

    pub fn command(&mut self, request: &str) -> Result<Vec<String>, String> {
        self.connection
            .write_all(format!("{request}\r").as_bytes())
            .and_then(|()| self.connection.flush())
            .map_err(|error| format!("writing `{request}`: {error}"))?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut response = std::mem::take(&mut self.pending);
        let mut buffer = [0_u8; 512];
        let mut searched = 0;
        let prompt_at = loop {
            if let Some(position) = response[searched..]
                .windows(PROMPT.len())
                .position(|window| window == PROMPT)
            {
                break searched + position;
            }
            if response.len() >= MAX_RESPONSE_BYTES {
                return Err(format!("`{request}` exceeded the response limit"));
            }
            if Instant::now() >= deadline {
                return Err(format!("`{request}` timed out waiting for the prompt"));
            }
            searched = response.len().saturating_sub(PROMPT.len() - 1);
            match self.connection.read(&mut buffer) {
                Ok(0) => return Err(format!("`{request}` ended before the prompt")),
                Ok(read) => response.extend_from_slice(&buffer[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(format!("reading `{request}`: {error}")),
            }
        };
        self.pending = response.split_off(prompt_at + PROMPT.len());
        response.truncate(prompt_at);
        let text = std::str::from_utf8(&response)
            .map_err(|error| format!("`{request}` returned non-UTF-8 data: {error}"))?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && *line != request)
            .map(str::to_owned)
            .collect())
    }

    pub fn silent(&mut self, request: &str) -> Result<(), String> {
        let response = self.command(request)?;
        if response.is_empty() {
            return Ok(());
        }
        Err(format!(
            "device refused `{request}`: {}",
            response.join(" ")
        ))
    }
}

pub fn reported_value(lines: &[String]) -> Option<&str> {
    if let Some(current) = lines
        .iter()
        .find_map(|line| line.strip_prefix("current:"))
        .or_else(|| lines.iter().find_map(|line| line.strip_prefix("power:")))
    {
        return Some(current.trim());
    }
    lines
        .iter()
        .find(|line| !line.starts_with("usage:") && !line.ends_with('?'))
        .map(String::as_str)
}

pub fn first_number(text: &str) -> Option<u64> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let rest = &text[start..];
    let end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

pub fn parse_frequencies(lines: Vec<String>) -> Result<Vec<u64>, String> {
    lines
        .into_iter()
        .map(|line| {
            line.split_whitespace()
                .next()
                .ok_or_else(|| "empty frequency row".to_owned())?
                .parse::<u64>()
                .map_err(|error| format!("invalid frequency row `{line}`: {error}"))
        })
        .collect()
}

pub fn parse_complex(lines: Vec<String>) -> Result<Vec<NanoVnaComplex>, String> {
    lines
        .into_iter()
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() != 2 {
                return Err(format!("invalid complex row `{line}`"));
            }
            let re = fields[0]
                .parse::<f64>()
                .map_err(|error| format!("invalid complex row `{line}`: {error}"))?;
            let im = fields[1]
                .parse::<f64>()
                .map_err(|error| format!("invalid complex row `{line}`: {error}"))?;
            if !re.is_finite() || !im.is_finite() {
                return Err(format!("non-finite complex row `{line}`"));
            }
            Ok(NanoVnaComplex { re, im })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct FixtureConnection {
        reads: VecDeque<u8>,
    }

    impl Read for FixtureConnection {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = buffer.len().min(self.reads.len()).min(2);
            for slot in &mut buffer[..read] {
                *slot = self.reads.pop_front().unwrap_or_default();
            }
            Ok(read)
        }
    }

    impl Write for FixtureConnection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_prompt_split_across_reads_still_frames_the_response() {
        let mut connection = FixtureConnection {
            reads: b"version\r\nNanoVNA-H4\r\nch> ".iter().copied().collect(),
        };
        let mut session = Session::new(&mut connection);
        assert_eq!(
            session.command("version"),
            Ok(vec!["NanoVNA-H4".to_owned()])
        );
    }

    #[test]
    fn settings_answers_are_read_bare_or_from_a_usage_line() {
        assert_eq!(
            reported_value(&[
                "usage: tcxo {Hz}".to_owned(),
                "current: 26000000".to_owned()
            ]),
            Some("26000000")
        );
        assert_eq!(
            reported_value(&[
                "usage: power {0-3}|{255 - auto}".to_owned(),
                "power: 255".to_owned()
            ]),
            Some("255")
        );
        assert_eq!(
            reported_value(&["0.000000000".to_owned()]),
            Some("0.000000000")
        );
        assert_eq!(reported_value(&["board_id?".to_owned()]), None);
    }

    #[test]
    fn inline_units_do_not_confuse_the_number_reader() {
        assert_eq!(first_number("4177 mV"), Some(4177));
        assert_eq!(first_number("bandwidth 3 (1000Hz)"), Some(3));
        assert_eq!(first_number("no digits here"), None);
    }

    #[test]
    fn complex_rows_reject_non_finite_values() {
        assert!(parse_complex(vec!["NaN 0".to_owned()]).is_err());
        assert!(parse_complex(vec!["0 inf".to_owned()]).is_err());
        assert!(parse_complex(vec!["0 1 2".to_owned()]).is_err());
    }
}
