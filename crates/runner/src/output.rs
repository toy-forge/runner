//! Turning a process's bytes into text as they arrive, and bounding how much
//! output a run may produce.

/// Decodes a stream of bytes into UTF-8 text, holding back a character that is
/// split across two reads until its last byte arrives. Bytes that are not
/// UTF-8 become U+FFFD.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    /// Decodes `bytes`, returning the text they complete.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    out.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
                    match error.error_len() {
                        Some(invalid) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.pending.drain(..valid + invalid);
                        }
                        None => {
                            // An incomplete character at the end: wait for the rest.
                            self.pending.drain(..valid);
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Returns what is still held, decoded lossily. Call once, at the end.
    pub fn finish(&mut self) -> String {
        let rest = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        rest
    }
}

/// Counts a run's output against the large-output guard.
///
/// * after `warn` bytes: [`Admit::warn`] is set once (the app may ask the user);
/// * past `limit` bytes: further bytes are dropped ([`Admit::keep`] is shorter
///   than the chunk) and [`Admit::truncated`] is set once.
#[derive(Debug, Clone)]
pub struct OutputGuard {
    total: u64,
    warn: Option<u64>,
    limit: u64,
    warned: bool,
    truncated: bool,
}

/// What to do with one chunk of output, see [`OutputGuard::admit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admit {
    /// How many leading bytes of the chunk to keep.
    pub keep: usize,
    /// The warning threshold was crossed by this chunk (reported once).
    pub warn: bool,
    /// The limit was crossed by this chunk (reported once).
    pub truncated: bool,
}

impl OutputGuard {
    /// A guard with an optional warning threshold and a hard limit.
    pub fn new(warn: Option<u64>, limit: u64) -> Self {
        Self {
            total: 0,
            warn,
            limit,
            warned: false,
            truncated: false,
        }
    }

    /// Bytes admitted so far.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether the limit was reached.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Changes the limit and the warning threshold (the user chose to continue).
    pub fn set_limits(&mut self, warn: Option<u64>, limit: u64) {
        self.warn = warn;
        self.limit = limit;
        if self.total < limit {
            self.truncated = false;
        }
    }

    /// Accounts for a chunk of `len` bytes.
    pub fn admit(&mut self, len: usize) -> Admit {
        let remaining = self.limit.saturating_sub(self.total);
        let keep = (len as u64).min(remaining) as usize;
        self.total += keep as u64;
        let truncated_now = keep < len && !self.truncated;
        if keep < len {
            self.truncated = true;
        }
        let warn_now = !self.warned && self.warn.is_some_and(|warn| self.total >= warn);
        if warn_now {
            self.warned = true;
        }
        Admit {
            keep,
            warn: warn_now,
            truncated: truncated_now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_warns_once_and_truncates_once() {
        let mut guard = OutputGuard::new(Some(5), 10);
        assert_eq!(
            guard.admit(4),
            Admit {
                keep: 4,
                warn: false,
                truncated: false
            }
        );
        assert_eq!(
            guard.admit(4),
            Admit {
                keep: 4,
                warn: true,
                truncated: false
            }
        );
        assert_eq!(
            guard.admit(4),
            Admit {
                keep: 2,
                warn: false,
                truncated: true
            }
        );
        assert_eq!(
            guard.admit(4),
            Admit {
                keep: 0,
                warn: false,
                truncated: false
            }
        );
        assert_eq!(guard.total(), 10);
        assert!(guard.is_truncated());
    }

    #[test]
    fn raising_the_limit_lets_output_continue() {
        let mut guard = OutputGuard::new(None, 4);
        assert_eq!(guard.admit(6).keep, 4);
        guard.set_limits(None, u64::MAX);
        assert_eq!(guard.admit(6).keep, 6);
        assert_eq!(guard.total(), 10);
    }

    #[test]
    fn passes_complete_text_through() {
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.push("hello\n".as_bytes()), "hello\n");
        assert_eq!(decoder.finish(), "");
    }

    #[test]
    fn holds_a_character_split_across_reads() {
        let bytes = "你好".as_bytes();
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.push(&bytes[..1]), "");
        assert_eq!(decoder.push(&bytes[1..4]), "你");
        assert_eq!(decoder.push(&bytes[4..]), "好");
    }

    #[test]
    fn replaces_invalid_bytes() {
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.push(b"a\xffb"), "a\u{FFFD}b");
    }

    #[test]
    fn flushes_an_unfinished_character_at_the_end() {
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.push(&"é".as_bytes()[..1]), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }
}
