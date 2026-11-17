//! What a piece of console text is: its stream and its severity, and the
//! theme-scope names those map to.

/// Which stream a range of text came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum Stream {
    /// The program's standard output.
    Stdout = 0,
    /// The program's standard error.
    Stderr = 1,
    /// Text the user (or the "Program Input" setting) sent to the program
    /// (typed input and its echo).
    Input = 2,
    /// Debugger output (`(lldb)`, `(Pdb)` prompts and replies).
    Debugger = 3,
    /// Messages from the app itself: compile status, "Run Succeeded", exit
    /// status, truncation notices.
    System = 4,
}

impl Stream {
    /// All streams, in discriminant order.
    pub const ALL: [Stream; 5] = [
        Stream::Stdout,
        Stream::Stderr,
        Stream::Input,
        Stream::Debugger,
        Stream::System,
    ];

    /// The theme scope of this stream (`console.stdout`, ...).
    pub fn scope(self) -> &'static str {
        match self {
            Stream::Stdout => "console.stdout",
            Stream::Stderr => "console.stderr",
            Stream::Input => "console.command.typed",
            Stream::Debugger => "console.debugger",
            Stream::System => "console.system",
        }
    }

    pub(crate) fn bit(self) -> u8 {
        1 << (self as u8)
    }

    /// The stream class for the "break the line when the stream changes"
    /// policy: stdout and input share a line (prompt + typed text).
    pub(crate) fn class(self) -> u8 {
        match self {
            Stream::Stdout | Stream::Input => 0,
            Stream::Stderr => 1,
            Stream::Debugger => 2,
            Stream::System => 3,
        }
    }
}

/// How severe a line is, for the "Errors" / "Errors and Warnings" filters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum Level {
    /// Nothing special.
    #[default]
    Normal,
    /// A note / hint (`console.notice`).
    Notice,
    /// A warning (`console.warning`).
    Warning,
    /// An error (`console.error`).
    Error,
}

impl Level {
    /// The theme scope (`console.error`, ...), `None` for [`Level::Normal`].
    pub fn scope(self) -> Option<&'static str> {
        match self {
            Level::Normal => None,
            Level::Notice => Some("console.notice"),
            Level::Warning => Some("console.warning"),
            Level::Error => Some("console.error"),
        }
    }

    pub(crate) fn bit(self) -> u8 {
        match self {
            Level::Normal => 0,
            Level::Error => 1 << 5,
            Level::Warning => 1 << 6,
            Level::Notice => 1 << 7,
        }
    }
}

/// A bit set summarising the scopes a line carries (streams of its runs plus
/// its level); filters are evaluated against it in O(1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct ScopeMask(pub u8);

impl ScopeMask {
    /// The mask of a line with these streams and this level.
    pub fn of(streams: impl IntoIterator<Item = Stream>, level: Level) -> ScopeMask {
        let mut m = level.bit();
        for s in streams {
            m |= s.bit();
        }
        ScopeMask(m)
    }

    /// Whether the mask contains `stream`.
    pub fn has_stream(self, stream: Stream) -> bool {
        self.0 & stream.bit() != 0
    }

    /// The scope names the mask stands for (always starting with `console`).
    pub fn scopes(self) -> Vec<&'static str> {
        let mut out = vec!["console"];
        for s in Stream::ALL {
            if self.has_stream(s) {
                out.push(s.scope());
            }
        }
        for l in [Level::Error, Level::Warning, Level::Notice] {
            if self.0 & l.bit() != 0 && let Some(s) = l.scope() {
                out.push(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_scopes() {
        let m = ScopeMask::of([Stream::Stdout, Stream::Input], Level::Warning);
        assert_eq!(
            m.scopes(),
            vec!["console", "console.stdout", "console.command.typed", "console.warning"]
        );
        assert_eq!(ScopeMask::default().scopes(), vec!["console"]);
    }
}
