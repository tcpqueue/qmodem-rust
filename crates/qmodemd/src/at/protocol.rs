use super::{AtError, ErrorKind, err};

pub fn default_flags() -> Vec<String> {
    ["OK", "ERROR", "+CMS ERROR:", "+CME ERROR:", "NO CARRIER"]
        .map(str::to_owned)
        .to_vec()
}

/// Upstream check_end_flags semantics: trim ASCII whitespace; an exact token or
/// a token followed by space/tab terminates, a substring embedded elsewhere does not.
pub fn end_match<'a>(line: &str, flags: &'a [String]) -> Option<&'a str> {
    let line = line.trim_matches([' ', '\t', '\r', '\n']);
    flags
        .iter()
        .find(|flag| {
            line == flag.as_str()
                || line
                    .strip_prefix(flag.as_str())
                    .is_some_and(|tail| tail.starts_with([' ', '\t']))
        })
        .map(String::as_str)
}
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
}
impl Decoder {
    pub fn clear(&mut self) {
        self.pending.clear();
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), AtError> {
        if self.pending.len() + bytes.len() > 64 * 1024 {
            return Err(err(ErrorKind::Overflow, "AT line exceeded limit"));
        }
        self.pending.extend_from_slice(bytes);
        Ok(())
    }
    pub fn next(&mut self, flags: &[String]) -> Result<Option<String>, AtError> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n' || *b == b'\r') {
                let bytes: Vec<u8> = self.pending.drain(..=end).collect();
                let line = String::from_utf8_lossy(&bytes[..end]).into_owned();
                if line.trim().is_empty() {
                    continue;
                }
                return Ok(Some(line));
            }
            if flags.iter().any(|f| f == ">")
                && self.pending.first() == Some(&b'>')
                && (self.pending.len() == 1 || self.pending.get(1) == Some(&b' '))
            {
                self.pending.drain(..1);
                if self.pending.first() == Some(&b' ') {
                    self.pending.drain(..1);
                }
                return Ok(Some(">".into()));
            }
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_boundaries_match_upstream() {
        let flags = default_flags();
        for line in [
            " OK\r\n",
            "ERROR",
            "+CME ERROR: 10",
            "+CMS ERROR:",
            "NO CARRIER",
            "OK extra",
        ] {
            assert!(end_match(line, &flags).is_some(), "{line}");
        }
        for line in [
            "BROKEN",
            "NOT OK",
            "ERRORISH",
            "+CME ERROR:10",
            "X+CMS ERROR: 1",
        ] {
            assert!(end_match(line, &flags).is_none(), "{line}");
        }
    }
    #[test]
    fn split_prompt_without_newline() {
        let mut d = Decoder::default();
        d.push(b"\r\n>").unwrap();
        assert_eq!(d.next(&[">".into()]).unwrap(), Some(">".into()));
    }
}
