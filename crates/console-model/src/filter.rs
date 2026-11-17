//! The console filter: the output menu (All / Program / Stdout / Stderr /
//! Debugger / Errors / ...) plus a text or regex field.

use crate::query::{Matcher, QueryError, SearchQuery};
use crate::scope::{ScopeSelector, SelectorError};

/// One entry of the "output" pop-up menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputFilter {
    /// Everything.
    AllOutput,
    /// `console.stdout, console.stderr`.
    ProgramOutput,
    /// `console.stdout`.
    ProgramStdout,
    /// `console.stderr`.
    ProgramStderr,
    /// `**.debugger`.
    DebuggerOutput,
    /// `console.error`.
    Errors,
    /// `console.error, console.warning, console.notice`.
    ErrorsAndWarnings,
    /// `-(console.error, console.warning, console.notice)`.
    HideErrorsAndWarnings,
    /// A user-entered scope selector ("Custom Output").
    Custom(String),
}

impl OutputFilter {
    /// The fixed menu entries in menu order (without "Custom Output").
    pub const MENU: [OutputFilter; 8] = [
        OutputFilter::AllOutput,
        OutputFilter::ProgramOutput,
        OutputFilter::ProgramStdout,
        OutputFilter::ProgramStderr,
        OutputFilter::DebuggerOutput,
        OutputFilter::Errors,
        OutputFilter::ErrorsAndWarnings,
        OutputFilter::HideErrorsAndWarnings,
    ];

    /// The menu title.
    pub fn title(&self) -> &str {
        match self {
            OutputFilter::AllOutput => "All Output",
            OutputFilter::ProgramOutput => "Program Output",
            OutputFilter::ProgramStdout => "Program Stdout",
            OutputFilter::ProgramStderr => "Program Stderr",
            OutputFilter::DebuggerOutput => "Debugger Output",
            OutputFilter::Errors => "Errors",
            OutputFilter::ErrorsAndWarnings => "Errors and Warnings",
            OutputFilter::HideErrorsAndWarnings => "Hide Errors and Warnings",
            OutputFilter::Custom(_) => "Custom Output",
        }
    }

    /// The scope selector this entry stands for; `None` for [`OutputFilter::AllOutput`].
    pub fn selector(&self) -> Option<&str> {
        match self {
            OutputFilter::AllOutput => None,
            OutputFilter::ProgramOutput => Some("console.stdout, console.stderr"),
            OutputFilter::ProgramStdout => Some("console.stdout"),
            OutputFilter::ProgramStderr => Some("console.stderr"),
            OutputFilter::DebuggerOutput => Some("**.debugger"),
            OutputFilter::Errors => Some("console.error"),
            OutputFilter::ErrorsAndWarnings => Some("console.error, console.warning, console.notice"),
            OutputFilter::HideErrorsAndWarnings => Some("-(console.error, console.warning, console.notice)"),
            OutputFilter::Custom(s) => Some(s),
        }
    }
}

/// A complete filter state: checked menu entries (combined by union; empty
/// means All Output) and the optional text field.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Filter {
    /// Checked output entries.
    pub outputs: Vec<OutputFilter>,
    /// The text / regex field; an empty text is ignored.
    pub text: Option<SearchQuery>,
}

/// A filter could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    /// A scope selector is invalid.
    #[error(transparent)]
    Selector(#[from] SelectorError),
    /// The text query is invalid.
    #[error(transparent)]
    Query(QueryError),
}

impl Filter {
    /// No filtering.
    pub fn all() -> Filter {
        Filter::default()
    }

    /// Only the given menu entry.
    pub fn output(o: OutputFilter) -> Filter {
        Filter {
            outputs: vec![o],
            text: None,
        }
    }

    /// Adds a text query.
    pub fn with_text(mut self, q: SearchQuery) -> Filter {
        self.text = Some(q);
        self
    }

    /// True when every line passes.
    pub fn is_trivial(&self) -> bool {
        self.outputs.iter().all(|o| *o == OutputFilter::AllOutput || o.selector().is_none())
            && self.text.as_ref().is_none_or(|t| t.text.is_empty())
    }

    /// The title for the pop-up button.
    pub fn title(&self) -> String {
        match self.outputs.as_slice() {
            [] => "All Output".into(),
            [one] => one.title().into(),
            _ => "Multiple Filters".into(),
        }
    }

    pub(crate) fn compile(&self) -> Result<CompiledFilter, FilterError> {
        let mut table = [false; 256];
        let selectors: Vec<&str> = self.outputs.iter().filter_map(|o| o.selector()).collect();
        if self.outputs.is_empty() || self.outputs.contains(&OutputFilter::AllOutput) {
            table = [true; 256];
        } else {
            for s in selectors {
                let t = ScopeSelector::parse(s)?.truth_table();
                for (slot, v) in table.iter_mut().zip(t) {
                    *slot |= v;
                }
            }
        }
        let text = match &self.text {
            Some(q) if !q.text.is_empty() => Some(Matcher::new(q).map_err(FilterError::Query)?),
            _ => None,
        };
        let identity = text.is_none() && table.iter().all(|b| *b);
        Ok(CompiledFilter { identity, table, text })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledFilter {
    pub(crate) identity: bool,
    pub(crate) table: [bool; 256],
    pub(crate) text: Option<Matcher>,
}

impl CompiledFilter {
    pub(crate) fn all() -> CompiledFilter {
        CompiledFilter {
            identity: true,
            table: [true; 256],
            text: None,
        }
    }

    pub(crate) fn passes(&self, mask: crate::tag::ScopeMask, text: &str) -> bool {
        self.table[mask.0 as usize] && self.text.as_ref().is_none_or(|m| m.is_match(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tag::{Level, ScopeMask, Stream};

    #[test]
    fn menu_selectors_parse() {
        for o in OutputFilter::MENU {
            let f = Filter::output(o.clone()).compile().unwrap();
            assert_eq!(f.identity, o == OutputFilter::AllOutput, "{o:?}");
        }
    }

    #[test]
    fn program_output_excludes_input_and_system() {
        let f = Filter::output(OutputFilter::ProgramOutput).compile().unwrap();
        let m = |s, l| ScopeMask::of([s], l);
        assert!(f.passes(m(Stream::Stdout, Level::Normal), ""));
        assert!(f.passes(m(Stream::Stderr, Level::Error), ""));
        assert!(!f.passes(m(Stream::Input, Level::Normal), ""));
        assert!(!f.passes(m(Stream::System, Level::Normal), ""));
        assert!(!f.passes(m(Stream::Debugger, Level::Normal), ""));
    }

    #[test]
    fn errors_and_hide() {
        let errs = Filter::output(OutputFilter::Errors).compile().unwrap();
        let hide = Filter::output(OutputFilter::HideErrorsAndWarnings).compile().unwrap();
        let e = ScopeMask::of([Stream::Stderr], Level::Error);
        let n = ScopeMask::of([Stream::Stdout], Level::Normal);
        assert!(errs.passes(e, "") && !errs.passes(n, ""));
        assert!(!hide.passes(e, "") && hide.passes(n, ""));
    }

    #[test]
    fn union_and_text() {
        let f = Filter {
            outputs: vec![OutputFilter::ProgramStderr, OutputFilter::DebuggerOutput],
            text: Some(SearchQuery::plain("boom")),
        }
        .compile()
        .unwrap();
        let err = ScopeMask::of([Stream::Stderr], Level::Normal);
        let dbg = ScopeMask::of([Stream::Debugger], Level::Normal);
        let out = ScopeMask::of([Stream::Stdout], Level::Normal);
        assert!(f.passes(err, "a BOOM"));
        assert!(!f.passes(err, "fine"));
        assert!(f.passes(dbg, "boom"));
        assert!(!f.passes(out, "boom"));
    }

    #[test]
    fn errors_surface() {
        assert!(matches!(
            Filter::output(OutputFilter::Custom("(".into())).compile(),
            Err(FilterError::Selector(_))
        ));
        let f = Filter::all().with_text(SearchQuery::regex("("));
        assert!(matches!(f.compile(), Err(FilterError::Query(_))));
        assert!(Filter::all().with_text(SearchQuery::plain("")).compile().unwrap().identity);
    }

    #[test]
    fn titles() {
        assert_eq!(Filter::all().title(), "All Output");
        assert_eq!(Filter::output(OutputFilter::Errors).title(), "Errors");
        assert!(Filter::all().is_trivial());
        assert!(!Filter::output(OutputFilter::Errors).is_trivial());
    }
}
