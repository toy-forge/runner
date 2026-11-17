//! Annotations: diagnostics placed in one buffer, and the per-document set
//! the editor shows (dedupe, ignore, dismiss, navigation, fixes).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use crate::ignore::IgnoreList;
use crate::model::{
    ColumnUnit, Diagnostic, Fix, Location, MatchedLocation, Origin, Severity, TextEdit, fnv64,
};
use crate::text::{EditSpan, LineIndex, TextPos, apply_edits, char_to_byte};

/// Stable identifier of an annotation: the same diagnostic at the same place
/// gets the same id on every lint run (it hashes file, severity, message,
/// message id and start line/column, not the producing diagnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnnotationId(pub u64);

impl std::fmt::Display for AnnotationId {
    /// 16 hex digits, usable as a link anchor.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl std::str::FromStr for AnnotationId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(s, 16).map(AnnotationId)
    }
}

/// A resolved edit: replace `range` (bytes) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEdit {
    /// Replaced range.
    pub range: Range<usize>,
    /// New text.
    pub text: String,
}

/// A fix with its edits placed in the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFix {
    /// Button title (`Fix` when the diagnostic gave none).
    pub title: String,
    /// Longer description.
    pub description: Option<String>,
    /// The edits, applied together.
    pub edits: Vec<ResolvedEdit>,
    /// Applied by "Apply Next/All Fixes" and auto-fix (see [`Fix::preferred`]).
    pub preferred: bool,
}

impl ResolvedFix {
    /// The edits as [`EditSpan`]s.
    pub fn spans(&self) -> Vec<EditSpan> {
        self.edits
            .iter()
            .map(|e| EditSpan {
                range: e.range.clone(),
                text: e.text.clone(),
            })
            .collect()
    }

    /// Start of the first edit (where "apply next fix" navigates).
    pub fn start(&self) -> Option<usize> {
        self.edits.iter().map(|e| e.range.start).min()
    }
}

/// A diagnostic placed in a buffer: what the editor underlines.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// Stable id.
    pub id: AnnotationId,
    /// Severity.
    pub severity: Severity,
    /// Run-time failure rather than compile-time problem.
    pub runtime: bool,
    /// Message (no severity prefix).
    pub message: String,
    /// Message identifier (ignore key).
    pub message_id: Option<String>,
    /// Id of the diagnostic entry that produced it.
    pub source: Option<String>,
    /// Origin.
    pub origin: Origin,
    /// `None` = this document; otherwise the file it is about (then `range`
    /// is `None`: it cannot be placed in this buffer).
    pub file: Option<PathBuf>,
    /// Byte range in the buffer; `None` when the diagnostic has no location
    /// in this document.
    pub range: Option<Range<usize>>,
    /// The location had no column: `range` is the whole line.
    pub whole_line: bool,
    /// Fixes.
    pub fixes: Vec<ResolvedFix>,
    /// Sub-messages (notes, stack frames).
    pub children: Vec<Annotation>,
    /// Console spans (for console-origin annotations).
    pub matched: Vec<MatchedLocation>,
}

impl Annotation {
    /// Key that ignores position: used to decide whether an issue is *new*
    /// (so typing above an old error does not count as a new one).
    pub fn issue_key(&self) -> u64 {
        fnv64(&[
            &[self.severity as u8, u8::from(self.runtime)],
            self.message.as_bytes(),
            self.message_id.as_deref().unwrap_or("").as_bytes(),
        ])
    }

    /// Key for dedupe: identical annotations from different sources collapse.
    pub fn dedupe_key(&self) -> AnnotationId {
        self.id
    }

    /// 0-based line of the start of the range.
    pub fn line(&self, index: &LineIndex<'_>) -> Option<usize> {
        self.range.as_ref().map(|r| index.line_of(r.start))
    }

    /// Start and end of the range as editor positions (0-based line, UTF-16
    /// column).
    pub fn positions(&self, index: &LineIndex<'_>) -> Option<(TextPos, TextPos)> {
        let r = self.range.as_ref()?;
        Some((index.position(r.start), index.position(r.end)))
    }

    /// The fix applied by default (single fix, or the one marked preferred).
    pub fn preferred_fix(&self) -> Option<&ResolvedFix> {
        if self.fixes.len() == 1 {
            return self.fixes.first();
        }
        self.fixes.iter().find(|f| f.preferred)
    }

    /// This annotation and all descendants, depth first.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Annotation>) {
        out.push(self);
        for child in &self.children {
            child.walk(out);
        }
    }

    fn map_ranges(&mut self, edit: &Range<usize>, new_len: usize) {
        if let Some(range) = &mut self.range {
            *range = map_range(range, edit, new_len);
        }
        // A fix whose edits touch the edited region is stale.
        let delta_touches = |r: &Range<usize>| {
            if r.start == r.end {
                edit.start <= r.start && r.start < edit.end
            } else {
                r.start < edit.end && edit.start < r.end
            }
        };
        self.fixes
            .retain(|f| !f.edits.iter().any(|e| delta_touches(&e.range)));
        for fix in &mut self.fixes {
            for e in &mut fix.edits {
                e.range = map_range(&e.range, edit, new_len);
            }
        }
        for child in &mut self.children {
            child.map_ranges(edit, new_len);
        }
    }
}

fn map_point(p: usize, edit: &Range<usize>, new_len: usize, is_end: bool) -> usize {
    if p < edit.start || (p == edit.start && !is_end) {
        p
    } else if p >= edit.end {
        p - (edit.end - edit.start) + new_len
    } else {
        // Inside the replaced text: start collapses to the edit start, end
        // to the end of the inserted text.
        if is_end { edit.start + new_len } else { edit.start }
    }
}

fn map_range(range: &Range<usize>, edit: &Range<usize>, new_len: usize) -> Range<usize> {
    let start = map_point(range.start, edit, new_len, false);
    let end = map_point(range.end, edit, new_len, true).max(start);
    start..end
}

/// Everything needed to place diagnostics in one document.
#[derive(Debug, Clone)]
pub struct DocumentContext<'a> {
    /// The buffer.
    pub text: &'a str,
    /// The document's path (`None` for an unsaved document).
    pub path: Option<&'a Path>,
    /// Other paths that denote the same document, typically the temporary
    /// copy a lint ran on.
    pub aliases: Vec<PathBuf>,
    /// Directory relative file names in the output are relative to.
    pub cwd: Option<&'a Path>,
}

impl<'a> DocumentContext<'a> {
    /// A context for `text` with no path information (every file name that
    /// is not clearly another file is taken to be the document).
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            path: None,
            aliases: Vec::new(),
            cwd: None,
        }
    }

    /// Sets the document path.
    #[must_use]
    pub fn path(mut self, path: &'a Path) -> Self {
        self.path = Some(path);
        self
    }

    /// Adds an alias path (the temp copy).
    #[must_use]
    pub fn alias(mut self, path: impl Into<PathBuf>) -> Self {
        self.aliases.push(path.into());
        self
    }

    /// Sets the directory relative names are relative to.
    #[must_use]
    pub fn cwd(mut self, cwd: &'a Path) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// Whether `file` (as printed by a tool) refers to this document.
    /// `None` always does.
    pub fn is_document(&self, file: Option<&Path>) -> bool {
        let Some(file) = file else { return true };
        let candidates = || self.path.into_iter().chain(self.aliases.iter().map(PathBuf::as_path));
        if self.path.is_none() && self.aliases.is_empty() {
            // Nothing to compare with: relative names are the document.
            return !file.is_absolute();
        }
        let absolute = if file.is_absolute() {
            file.to_path_buf()
        } else if let Some(cwd) = self.cwd {
            cwd.join(file)
        } else {
            file.to_path_buf()
        };
        if candidates().any(|c| same_file(c, &absolute)) {
            return true;
        }
        // A bare relative name that is the document's file name: the tool ran
        // in the document's directory.
        if !file.is_absolute() {
            let comps: Vec<_> = file
                .components()
                .filter(|c| !matches!(c, Component::CurDir))
                .collect();
            if comps.len() == 1
                && let Some(name) = comps[0].as_os_str().to_str()
            {
                return candidates().any(|c| c.file_name().and_then(|n| n.to_str()) == Some(name));
            }
        }
        false
    }
}

/// Whether two paths denote the same file (canonical when both exist, else
/// lexical).
pub fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) {
        return ca == cb;
    }
    lexical_normalize(a) == lexical_normalize(b)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Length in bytes of the token at the start of `rest`: a quoted string, an
/// identifier-like run, or nothing (callers widen to one character).
fn token_len(rest: &str) -> usize {
    let mut chars = rest.char_indices();
    match chars.next() {
        Some((_, q @ ('"' | '\'' | '`'))) => {
            let mut escaped = false;
            for (i, c) in chars {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    return i + 1;
                }
            }
            0
        }
        _ => rest
            .chars()
            .take_while(|c| is_ident(*c))
            .map(char::len_utf8)
            .sum(),
    }
}

/// The result of placing a location in a buffer.
struct Placed {
    range: Range<usize>,
    whole_line: bool,
}

fn place(loc: &Location, code: Option<&str>, index: &LineIndex<'_>) -> Option<Placed> {
    let text = index.text();
    match loc {
        Location::Lines {
            line,
            column,
            end_line,
            end_column,
            unit,
        } => {
            let line = *line as usize;
            let bytes = *unit == ColumnUnit::Bytes;
            let line_range = index.line_range(line.checked_sub(1)?)?;
            let Some(column) = column else {
                if let Some(code) = code.filter(|c| !c.is_empty())
                    && let Some(pos) = text[line_range.clone()].find(code)
                {
                    let start = line_range.start + pos;
                    return Some(Placed {
                        range: start..start + code.len(),
                        whole_line: false,
                    });
                }
                return Some(Placed {
                    range: line_range,
                    whole_line: true,
                });
            };
            let start = index.offset_of(line, *column as usize, bytes)?;
            let end = match (end_line, end_column) {
                (_, Some(ec)) => {
                    let el = end_line.map_or(line, |l| l as usize);
                    index.offset_of(el, *ec as usize, bytes).unwrap_or(start)
                }
                (Some(el), None) => index
                    .line_range((*el as usize).checked_sub(1)?)
                    .map_or(start, |r| r.end),
                (None, None) => {
                    let rest = &text[start..line_range.end.max(start)];
                    if let Some(code) = code.filter(|c| rest.starts_with(*c)) {
                        start + code.len()
                    } else {
                        start + token_len(rest)
                    }
                }
            };
            Some(Placed {
                range: widen(start.min(end), end.max(start), index, line_range),
                whole_line: false,
            })
        }
        Location::Offsets {
            offset,
            length,
            bytes,
        } => {
            let (start, end) = if *bytes {
                (
                    (*offset).min(text.len()),
                    offset.saturating_add(*length).min(text.len()),
                )
            } else {
                let s = char_to_byte(text, *offset);
                let e = char_to_byte(text, offset.saturating_add(*length));
                (s, e)
            };
            let start = crate::text::floor_boundary(text, start);
            let end = crate::text::floor_boundary(text, end).max(start);
            let line_range = index.line_range(index.line_of(start))?;
            Some(Placed {
                range: widen(start, end, index, line_range),
                whole_line: false,
            })
        }
        Location::InLine {
            line,
            text: printed,
            offset,
            length,
        } => {
            let line_range = index.line_range((*line as usize).checked_sub(1)?)?;
            let line_text = &text[line_range.clone()];
            let base = line_text
                .find(printed.as_str())
                .or_else(|| line_text.find(printed.trim_end()));
            let Some(base) = base.filter(|_| !printed.is_empty()) else {
                return Some(Placed {
                    range: line_range,
                    whole_line: true,
                });
            };
            let start = line_range.start + base + char_to_byte(printed, *offset);
            let end_chars = if *length == 0 {
                printed.chars().count()
            } else {
                offset + length
            };
            let end = line_range.start + base + char_to_byte(printed, end_chars);
            Some(Placed {
                range: widen(start, end.max(start), index, line_range),
                whole_line: false,
            })
        }
    }
}

/// Makes an empty range one character wide (the previous character at the
/// end of a line, so the underline has something to cover).
fn widen(start: usize, end: usize, index: &LineIndex<'_>, line: Range<usize>) -> Range<usize> {
    if end > start {
        return start..end;
    }
    let text = index.text();
    if let Some(ch) = text[start..line.end.max(start)].chars().next() {
        return start..start + ch.len_utf8();
    }
    match text[line.start..start].chars().next_back() {
        Some(ch) => start - ch.len_utf8()..start,
        None => start..start,
    }
}

/// Resolves the location of an *edit* exactly: a column without an end is an
/// insertion point, no column is the whole line.
fn place_edit(loc: &Location, index: &LineIndex<'_>) -> Option<Range<usize>> {
    match loc {
        Location::Lines {
            line,
            column,
            end_line,
            end_column,
            unit,
        } => {
            let line = *line as usize;
            let bytes = *unit == ColumnUnit::Bytes;
            let Some(column) = column else {
                return index.line_range(line.checked_sub(1)?);
            };
            let start = index.offset_of(line, *column as usize, bytes)?;
            let end = match end_column {
                Some(ec) => index
                    .offset_of(end_line.map_or(line, |l| l as usize), *ec as usize, bytes)
                    .unwrap_or(start),
                None => start,
            };
            Some(start..end.max(start))
        }
        Location::Offsets {
            offset,
            length,
            bytes,
        } => {
            let text = index.text();
            let (s, e) = if *bytes {
                (
                    (*offset).min(text.len()),
                    offset.saturating_add(*length).min(text.len()),
                )
            } else {
                (
                    char_to_byte(text, *offset),
                    char_to_byte(text, offset.saturating_add(*length)),
                )
            };
            Some(s..e.max(s))
        }
        Location::InLine { .. } => place(loc, None, index).map(|p| p.range),
    }
}

fn shorten(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' { '\u{23ce}' } else { c }).collect();
    if flat.chars().count() > 24 {
        let head: String = flat.chars().take(23).collect();
        format!("{head}\u{2026}")
    } else {
        flat
    }
}

/// The title CodeRunner derives for a single-edit fix without a title:
/// `Insert '%@'`, `Remove '%@'`, `Replace '%@' with '%@'`.
fn auto_title(edit: &ResolvedEdit, text: &str) -> String {
    let old = text.get(edit.range.clone()).unwrap_or_default();
    match (old.is_empty(), edit.text.is_empty()) {
        (true, true) => "Fix".to_owned(),
        (true, false) => format!("Insert '{}'", shorten(&edit.text)),
        (false, true) => format!("Remove '{}'", shorten(old)),
        (false, false) => format!("Replace '{}' with '{}'", shorten(old), shorten(&edit.text)),
    }
}

fn resolve_fix(fix: &Fix, single: bool, index: &LineIndex<'_>) -> Option<ResolvedFix> {
    let mut edits = Vec::new();
    for TextEdit {
        location,
        replacement,
    } in &fix.edits
    {
        edits.push(ResolvedEdit {
            range: place_edit(location, index)?,
            text: replacement.clone(),
        });
    }
    if edits.is_empty() {
        return None;
    }
    let title = match fix.title.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => t.to_owned(),
        None if edits.len() == 1 => auto_title(&edits[0], index.text()),
        None => "Fix".to_owned(),
    };
    Some(ResolvedFix {
        title,
        description: fix.description.clone(),
        edits,
        preferred: fix.preferred || single,
    })
}

/// Places a diagnostic (and its children) in the document `ctx`.
///
/// Diagnostics about another file resolve to an annotation with `file` set
/// and no range (they are kept so the UI can offer "open that file"); callers
/// that only show this document's annotations use [`resolve_all`].
pub fn resolve(diag: &Diagnostic, ctx: &DocumentContext<'_>) -> Annotation {
    let index = LineIndex::new(ctx.text);
    resolve_with(diag, ctx, &index)
}

fn resolve_with(diag: &Diagnostic, ctx: &DocumentContext<'_>, index: &LineIndex<'_>) -> Annotation {
    let here = ctx.is_document(diag.file.as_deref());
    let placed = if here {
        diag.location
            .as_ref()
            .and_then(|l| place(l, diag.code.as_deref(), index))
    } else {
        None
    };
    let single = diag.fixes.len() == 1;
    let fixes = if here {
        diag.fixes
            .iter()
            .filter_map(|f| resolve_fix(f, single, index))
            .collect()
    } else {
        Vec::new()
    };
    let (range, whole_line) = match placed {
        Some(p) => (Some(p.range), p.whole_line),
        None => (None, false),
    };
    let file = if here { None } else { diag.file.clone() };
    let (line, col) = range.as_ref().map_or((0, 0), |r| {
        let line = index.line_of(r.start);
        let start = index.line_range(line).map_or(0, |l| l.start);
        (line + 1, r.start - start)
    });
    let line_s = line.to_string();
    let col_s = col.to_string();
    let file_s = file
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = AnnotationId(fnv64(&[
        file_s.as_bytes(),
        &[diag.severity as u8, u8::from(diag.runtime)],
        diag.message.as_bytes(),
        diag.message_id.as_deref().unwrap_or("").as_bytes(),
        line_s.as_bytes(),
        col_s.as_bytes(),
    ]));
    Annotation {
        id,
        severity: diag.severity,
        runtime: diag.runtime,
        message: diag.message.clone(),
        message_id: diag.message_id.clone(),
        source: diag.source.clone(),
        origin: diag.origin,
        file,
        range,
        whole_line,
        fixes,
        children: diag
            .children
            .iter()
            .map(|c| {
                let mut c = c.clone();
                // A child without a file is in the parent's file.
                if c.file.is_none() {
                    c.file.clone_from(&diag.file);
                }
                resolve_with(&c, ctx, index)
            })
            .collect(),
        matched: diag.matched.clone(),
    }
}

/// Places all diagnostics that are about this document; the others are
/// returned separately.
pub fn resolve_all(
    diags: &[Diagnostic],
    ctx: &DocumentContext<'_>,
) -> (Vec<Annotation>, Vec<Annotation>) {
    let index = LineIndex::new(ctx.text);
    let mut here = Vec::new();
    let mut foreign = Vec::new();
    for d in diags {
        let a = resolve_with(d, ctx, &index);
        if a.file.is_none() && a.range.is_some() {
            here.push(a);
        } else if a.file.is_some() {
            foreign.push(a);
        } else {
            // About this document but without a usable location: show it on
            // the first line so it is not lost.
            let mut a = a;
            if let Some(r) = index.line_range(0) {
                a.range = Some(r);
                a.whole_line = true;
            }
            here.push(a);
        }
    }
    (here, foreign)
}

/// Counts per severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Errors (including run-time errors).
    pub errors: usize,
    /// Warnings.
    pub warnings: usize,
    /// Notices.
    pub notices: usize,
}

impl Counts {
    /// Sum of all.
    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.notices
    }

    /// The highest severity present.
    pub fn worst(&self) -> Option<Severity> {
        if self.errors > 0 {
            Some(Severity::Error)
        } else if self.warnings > 0 {
            Some(Severity::Warning)
        } else if self.notices > 0 {
            Some(Severity::Notice)
        } else {
            None
        }
    }

    fn add(&mut self, severity: Severity) {
        match severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
            Severity::Notice => self.notices += 1,
        }
    }
}

/// What a replacement changed (input of the scheduler's "cancel run on new
/// issues" decision).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeReport {
    /// Annotations that were not present before (by issue key, ignoring
    /// position and ignoring ignored/dismissed ones).
    pub new: Counts,
    /// Ids of the annotations now present from that source.
    pub ids: Vec<AnnotationId>,
}

/// Which annotations a navigation command considers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavFilter {
    /// Every visible annotation ("Jump To Next Diagnostic").
    Any,
    /// Only errors ("Jump To Next Error").
    ErrorsOnly,
}

impl NavFilter {
    fn accepts(self, a: &Annotation) -> bool {
        match self {
            NavFilter::Any => true,
            NavFilter::ErrorsOnly => a.severity == Severity::Error,
        }
    }
}

/// The edits of one "apply fix" command. Apply them with
/// [`FixPlan::apply_to`], or pass `edits` to an editor as one undo step
/// (they never overlap; they are sorted by position).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixPlan {
    /// Annotations whose fixes are included.
    pub annotations: Vec<AnnotationId>,
    /// Titles of the applied fixes.
    pub titles: Vec<String>,
    /// Non-overlapping edits sorted by start.
    pub edits: Vec<EditSpan>,
}

impl FixPlan {
    /// Whether there is nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// The edits sorted from the end of the text to the start, which can be
    /// applied one by one without recomputing offsets.
    pub fn edits_descending(&self) -> Vec<EditSpan> {
        let mut e = self.edits.clone();
        e.reverse();
        e
    }

    /// Applies the plan to `text`.
    pub fn apply_to(&self, text: &str) -> String {
        apply_edits(text, &self.edits)
    }

    fn push(&mut self, id: AnnotationId, fix: &ResolvedFix) -> bool {
        let spans = fix.spans();
        let overlaps = |a: &Range<usize>, b: &Range<usize>| {
            a.start < b.end && b.start < a.end || a == b && a.start == a.end
        };
        if spans.iter().any(|n| {
            self.edits
                .iter()
                .any(|o| overlaps(&n.range, &o.range))
        }) {
            return false;
        }
        self.annotations.push(id);
        self.titles.push(fix.title.clone());
        self.edits.extend(spans);
        self.edits.sort_by_key(|e| (e.range.start, e.range.end));
        true
    }
}

/// The annotations of one document, from all diagnostic sources.
///
/// Sources are replaced as a unit ([`AnnotationSet::replace_source`]); the
/// visible view hides dismissed and ignored annotations and collapses
/// duplicates reported by several sources.
#[derive(Debug, Clone, Default)]
pub struct AnnotationSet {
    items: Vec<Annotation>,
    dismissed: HashSet<AnnotationId>,
    global_ignore: IgnoreList,
    document_ignore: IgnoreList,
}

impl AnnotationSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the globally ignored messages (`Ignored Messages...`).
    pub fn set_global_ignore(&mut self, list: IgnoreList) {
        self.global_ignore = list;
    }

    /// Sets the ignored messages of this document.
    pub fn set_document_ignore(&mut self, list: IgnoreList) {
        self.document_ignore = list;
    }

    /// The document's ignore list.
    pub fn document_ignore(&self) -> &IgnoreList {
        &self.document_ignore
    }

    /// The global ignore list.
    pub fn global_ignore(&self) -> &IgnoreList {
        &self.global_ignore
    }

    fn hidden(&self, a: &Annotation) -> bool {
        self.dismissed.contains(&a.id)
            || self.global_ignore.matches(&a.message, a.message_id.as_deref())
            || self.document_ignore.matches(&a.message, a.message_id.as_deref())
    }

    /// Replaces everything produced by diagnostic `source` with `new` and
    /// reports what is new. Passing the console origin replaces the console
    /// annotations of that source.
    pub fn replace_source(
        &mut self,
        source: &str,
        origin: Origin,
        new: Vec<Annotation>,
    ) -> ChangeReport {
        let mut old_keys: HashMap<u64, usize> = HashMap::new();
        for a in self
            .items
            .iter()
            .filter(|a| a.source.as_deref() == Some(source) && a.origin == origin)
        {
            *old_keys.entry(a.issue_key()).or_default() += 1;
        }
        self.items
            .retain(|a| !(a.source.as_deref() == Some(source) && a.origin == origin));
        let mut report = ChangeReport::default();
        for mut a in new {
            a.source = Some(source.to_owned());
            a.origin = origin;
            if !self.hidden(&a) {
                match old_keys.get_mut(&a.issue_key()) {
                    Some(n) if *n > 0 => *n -= 1,
                    _ => report.new.add(a.severity),
                }
            }
            report.ids.push(a.id);
            self.items.push(a);
        }
        self.sort();
        report
    }

    /// Removes all annotations of a source (`Clear Diagnostics`).
    pub fn clear_source(&mut self, source: &str) {
        self.items.retain(|a| a.source.as_deref() != Some(source));
    }

    /// Removes the annotations of one origin.
    pub fn clear_origin(&mut self, origin: Origin) {
        self.items.retain(|a| a.origin != origin);
    }

    /// Removes everything (dismissals are kept).
    pub fn clear(&mut self) {
        self.items.clear();
    }

    fn sort(&mut self) {
        self.items.sort_by(|a, b| {
            let ka = a.range.as_ref().map_or(0, |r| r.start);
            let kb = b.range.as_ref().map_or(0, |r| r.start);
            ka.cmp(&kb)
                .then(b.severity.cmp(&a.severity))
                .then(a.id.cmp(&b.id))
        });
    }

    /// Visible annotations in document order: no dismissed or ignored ones,
    /// duplicates collapsed, only those placed in this document.
    pub fn visible(&self) -> Vec<&Annotation> {
        let mut seen = HashSet::new();
        self.items
            .iter()
            .filter(|a| a.range.is_some() && !self.hidden(a) && seen.insert(a.dedupe_key()))
            .collect()
    }

    /// Number of visible annotations per severity.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for a in self.visible() {
            c.add(a.severity);
        }
        c
    }

    /// Looks an annotation (or a child) up by id.
    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        let mut all = Vec::new();
        for a in &self.items {
            a.walk(&mut all);
        }
        all.into_iter().find(|a| a.id == id)
    }

    /// Visible annotations whose range contains `offset` (or touches it when
    /// empty), most severe first.
    pub fn at_offset(&self, offset: usize) -> Vec<&Annotation> {
        let mut v: Vec<&Annotation> = self
            .visible()
            .into_iter()
            .filter(|a| {
                a.range
                    .as_ref()
                    .is_some_and(|r| (r.start <= offset && offset < r.end) || (r.start == r.end && r.start == offset))
            })
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.severity));
        v
    }

    /// Visible annotations that start on `line` (0-based).
    pub fn on_line(&self, line: usize, index: &LineIndex<'_>) -> Vec<&Annotation> {
        self.visible()
            .into_iter()
            .filter(|a| a.line(index) == Some(line))
            .collect()
    }

    /// The next annotation whose start is after `caret` (byte offset),
    /// wrapping to the first one when there is none after.
    pub fn next_from(&self, caret: usize, filter: NavFilter) -> Option<&Annotation> {
        let list: Vec<&Annotation> = self
            .visible()
            .into_iter()
            .filter(|a| filter.accepts(a))
            .collect();
        list.iter()
            .find(|a| a.range.as_ref().is_some_and(|r| r.start > caret))
            .or_else(|| list.first())
            .copied()
    }

    /// The previous annotation whose start is before `caret`, wrapping to the
    /// last one.
    pub fn previous_from(&self, caret: usize, filter: NavFilter) -> Option<&Annotation> {
        let list: Vec<&Annotation> = self
            .visible()
            .into_iter()
            .filter(|a| filter.accepts(a))
            .collect();
        list.iter()
            .rev()
            .find(|a| a.range.as_ref().is_some_and(|r| r.start < caret))
            .or_else(|| list.last())
            .copied()
    }

    /// Whether a navigation command can do something (menu enablement).
    pub fn can_navigate(&self, filter: NavFilter) -> bool {
        self.visible().iter().any(|a| filter.accepts(a))
    }

    /// Whether there is any fix to apply (menu enablement of the fix items).
    pub fn has_fixes(&self) -> bool {
        self.fix_candidates().next().is_some()
    }

    fn fix_candidates(&self) -> impl Iterator<Item = (AnnotationId, &ResolvedFix)> {
        let mut seen = HashSet::new();
        let mut all: Vec<&Annotation> = Vec::new();
        for a in self.items.iter().filter(|a| !self.hidden(a) && a.range.is_some()) {
            a.walk(&mut all);
        }
        all.into_iter()
            .filter(move |a| seen.insert(a.id))
            .filter_map(|a| a.preferred_fix().map(|f| (a.id, f)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// "Apply Next Fix": the preferred fix of the first annotation with a fix
    /// at or after `caret` (wrapping).
    pub fn next_fix(&self, caret: usize) -> FixPlan {
        let mut cands: Vec<(AnnotationId, &ResolvedFix)> = self.fix_candidates().collect();
        cands.sort_by_key(|(_, f)| f.start().unwrap_or(0));
        let pick = cands
            .iter()
            .find(|(_, f)| f.start().is_some_and(|s| s >= caret))
            .or_else(|| cands.first());
        let mut plan = FixPlan::default();
        if let Some((id, fix)) = pick {
            plan.push(*id, fix);
        }
        plan
    }

    /// "Apply All Fixes": the preferred fix of every annotation, skipping
    /// fixes that overlap an earlier one.
    pub fn all_fixes(&self) -> FixPlan {
        let mut plan = FixPlan::default();
        let mut cands: Vec<(AnnotationId, &ResolvedFix)> = self.fix_candidates().collect();
        cands.sort_by_key(|(_, f)| f.start().unwrap_or(0));
        for (id, fix) in cands {
            plan.push(id, fix);
        }
        plan
    }

    /// The plan for one chosen fix (a fix button) of annotation `id`.
    pub fn fix_plan(&self, id: AnnotationId, fix_index: usize) -> Option<FixPlan> {
        let a = self.get(id)?;
        let fix = a.fixes.get(fix_index)?;
        let mut plan = FixPlan::default();
        plan.push(id, fix);
        Some(plan)
    }

    /// The fixes of annotations matched by auto-fix rules, to apply before a
    /// run ([`crate::AutoFixRules`]).
    pub fn auto_fixes(&self, rules: &crate::ignore::AutoFixRules) -> FixPlan {
        let mut plan = FixPlan::default();
        let mut cands: Vec<(AnnotationId, &ResolvedFix, &Annotation)> = Vec::new();
        let mut seen = HashSet::new();
        for a in self.visible() {
            let mut all = Vec::new();
            a.walk(&mut all);
            for n in all {
                if seen.insert(n.id)
                    && rules.matches(&n.message, n.message_id.as_deref())
                    && let Some(f) = n.preferred_fix()
                {
                    cands.push((n.id, f, n));
                }
            }
        }
        cands.sort_by_key(|(_, f, _)| f.start().unwrap_or(0));
        for (id, fix, _) in cands {
            plan.push(id, fix);
        }
        plan
    }

    /// Hides one annotation until the next lint ("Dismiss Message").
    pub fn dismiss(&mut self, id: AnnotationId) {
        self.dismissed.insert(id);
    }

    /// Hides everything currently shown ("Dismiss All Messages In Document").
    pub fn dismiss_all(&mut self) {
        let ids: Vec<AnnotationId> = self.visible().iter().map(|a| a.id).collect();
        self.dismissed.extend(ids);
    }

    /// Forgets all dismissals.
    pub fn undismiss_all(&mut self) {
        self.dismissed.clear();
    }

    /// Maps all ranges through a text edit: `range` (bytes, in the old text)
    /// was replaced by `new_len` bytes. Annotations whose range is touched
    /// are resized; fixes touched by the edit are dropped (they are stale).
    pub fn map_edit(&mut self, range: Range<usize>, new_len: usize) {
        for a in &mut self.items {
            a.map_ranges(&range, new_len);
        }
        self.sort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Diagnostic, Fix, Location, Severity};

    fn diag(sev: Severity, line: u32, col: u32, msg: &str) -> Diagnostic {
        Diagnostic::new(sev, msg).at(Location::line_col(line, col))
    }

    #[test]
    fn resolves_columns_and_words() {
        let text = "int main() {\n  retrun 0;\n}\n";
        let ctx = DocumentContext::new(text);
        let a = resolve(&diag(Severity::Error, 2, 3, "x"), &ctx);
        assert_eq!(&text[a.range.clone().unwrap()], "retrun");
        let whole = resolve(
            &Diagnostic::new(Severity::Error, "y").at(Location::line(2)),
            &ctx,
        );
        assert!(whole.whole_line);
        assert_eq!(&text[whole.range.unwrap()], "  retrun 0;");
        let past = resolve(&diag(Severity::Error, 99, 1, "z"), &ctx);
        assert!(past.range.is_none());
    }

    #[test]
    fn quoted_strings_are_one_token() {
        let text = "x = \"a \\\" b\" + 1\n";
        let ctx = DocumentContext::new(text);
        let a = resolve(&diag(Severity::Error, 1, 5, "x"), &ctx);
        assert_eq!(&text[a.range.unwrap()], "\"a \\\" b\"");
        let unterminated = "x = \"abc\n";
        let a = resolve(&diag(Severity::Error, 1, 5, "x"), &DocumentContext::new(unterminated));
        assert_eq!(&unterminated[a.range.unwrap()], "\"");
    }

    #[test]
    fn empty_column_range_at_line_end_covers_previous_char() {
        let text = "ab\n";
        let ctx = DocumentContext::new(text);
        let a = resolve(&diag(Severity::Error, 1, 3, "x"), &ctx);
        assert_eq!(&text[a.range.unwrap()], "b");
    }

    #[test]
    fn offsets_and_inline_locations() {
        let text = "x = 1\n    foo bar\n";
        let ctx = DocumentContext::new(text);
        let off = resolve(
            &Diagnostic::new(Severity::Error, "o").at(Location::Offsets {
                offset: 4,
                length: 1,
                bytes: false,
            }),
            &ctx,
        );
        assert_eq!(&text[off.range.unwrap()], "1");
        let inl = resolve(
            &Diagnostic::new(Severity::Error, "i").at(Location::InLine {
                line: 2,
                text: "foo bar".into(),
                offset: 4,
                length: 3,
            }),
            &ctx,
        );
        assert_eq!(&text[inl.range.unwrap()], "bar");
    }

    #[test]
    fn stable_ids_and_dedupe() {
        let text = "a\nb\n";
        let ctx = DocumentContext::new(text);
        let d = diag(Severity::Warning, 1, 1, "same");
        let a1 = resolve(&d, &ctx);
        let a2 = resolve(&d, &ctx);
        assert_eq!(a1.id, a2.id);
        let mut set = AnnotationSet::new();
        set.replace_source("clang", Origin::Document, vec![a1]);
        set.replace_source("compile", Origin::Document, vec![a2]);
        assert_eq!(set.visible().len(), 1, "same diagnostic from two sources shows once");
        assert_eq!(set.counts().warnings, 1);
    }

    #[test]
    fn replace_reports_new_issues_by_message_not_position() {
        let text = "a\nb\nc\n";
        let ctx = DocumentContext::new(text);
        let mut set = AnnotationSet::new();
        let r1 = set.replace_source(
            "s",
            Origin::Document,
            vec![resolve(&diag(Severity::Error, 1, 1, "boom"), &ctx)],
        );
        assert_eq!(r1.new.errors, 1);
        let r2 = set.replace_source(
            "s",
            Origin::Document,
            vec![
                resolve(&diag(Severity::Error, 2, 1, "boom"), &ctx),
                resolve(&diag(Severity::Warning, 3, 1, "meh"), &ctx),
            ],
        );
        assert_eq!(r2.new, Counts { errors: 0, warnings: 1, notices: 0 });
    }

    #[test]
    fn navigation_wraps() {
        let text = "aa\nbb\ncc\ndd\n";
        let ctx = DocumentContext::new(text);
        let mut set = AnnotationSet::new();
        set.replace_source(
            "s",
            Origin::Document,
            vec![
                resolve(&diag(Severity::Warning, 1, 1, "w"), &ctx),
                resolve(&diag(Severity::Error, 3, 1, "e"), &ctx),
                resolve(&diag(Severity::Notice, 4, 1, "n"), &ctx),
            ],
        );
        let start = |a: Option<&Annotation>| a.and_then(|a| a.range.as_ref().map(|r| r.start));
        assert_eq!(start(set.next_from(0, NavFilter::Any)), Some(6));
        assert_eq!(start(set.next_from(6, NavFilter::Any)), Some(9));
        assert_eq!(start(set.next_from(9, NavFilter::Any)), Some(0), "wraps");
        assert_eq!(start(set.next_from(0, NavFilter::ErrorsOnly)), Some(6));
        assert_eq!(start(set.previous_from(0, NavFilter::Any)), Some(9), "wraps back");
        assert_eq!(start(set.previous_from(9, NavFilter::Any)), Some(6));
        assert_eq!(start(set.previous_from(6, NavFilter::ErrorsOnly)), Some(6));
        assert!(AnnotationSet::new().next_from(0, NavFilter::Any).is_none());
    }

    #[test]
    fn dismiss_and_ignore_hide() {
        let text = "a\nb\n";
        let ctx = DocumentContext::new(text);
        let mut set = AnnotationSet::new();
        let a = resolve(&diag(Severity::Error, 1, 1, "unused variable 'x'"), &ctx);
        let id = a.id;
        let b = resolve(&diag(Severity::Warning, 2, 1, "other"), &ctx);
        set.replace_source("s", Origin::Document, vec![a, b]);
        assert_eq!(set.counts().total(), 2);
        set.dismiss(id);
        assert_eq!(set.counts().total(), 1);
        set.undismiss_all();
        set.set_document_ignore(IgnoreList::from_text("unused variable"));
        assert_eq!(set.counts(), Counts { errors: 0, warnings: 1, notices: 0 });
        set.dismiss_all();
        assert_eq!(set.counts().total(), 0);
    }

    #[test]
    fn fixes_next_all_and_choice() {
        let text = "let x = 1\nlet y = 2\n";
        let ctx = DocumentContext::new(text);
        let fix_semi = |line: u32, col: u32| {
            Fix::single(
                "Insert ';'",
                Location::line_col(line, col),
                ";",
            )
        };
        let d1 = diag(Severity::Error, 1, 10, "expected ;").with_fix(fix_semi(1, 10));
        let d2 = diag(Severity::Error, 2, 10, "expected ;").with_fix(fix_semi(2, 10));
        // Alternatives, none preferred: never applied in bulk.
        let d3 = diag(Severity::Error, 1, 1, "conflict")
            .with_fix(Fix::single("A", Location::line_col(1, 1), "A"))
            .with_fix(Fix::single("B", Location::line_col(1, 1), "B"));
        let mut set = AnnotationSet::new();
        let res: Vec<Annotation> = [d1, d2, d3].iter().map(|d| resolve(d, &ctx)).collect();
        let conflict_id = res[2].id;
        set.replace_source("s", Origin::Document, res);
        assert!(set.has_fixes());
        let next = set.next_fix(0);
        assert_eq!(next.titles, ["Insert ';'"]);
        assert_eq!(next.apply_to(text), "let x = 1;\nlet y = 2\n");
        let after = set.next_fix(11);
        assert_eq!(after.apply_to(text), "let x = 1\nlet y = 2;\n");
        let all = set.all_fixes();
        assert_eq!(all.apply_to(text), "let x = 1;\nlet y = 2;\n");
        let b = set.fix_plan(conflict_id, 1).unwrap();
        assert_eq!(b.apply_to(text), "Blet x = 1\nlet y = 2\n");
    }

    #[test]
    fn auto_fix_rules_select_fixes() {
        let text = "a b\nc d\n";
        let ctx = DocumentContext::new(text);
        let fix = |l, c| Fix::single("F", Location::line_col(l, c), "!");
        let d1 = diag(Severity::Warning, 1, 1, "class 'A' lacks a base class").with_id("-Wobjc-root-class").with_fix(fix(1, 1));
        let d2 = diag(Severity::Warning, 2, 1, "unused x").with_id("-Wunused").with_fix(fix(2, 1));
        let mut set = AnnotationSet::new();
        set.replace_source("s", Origin::Document, vec![resolve(&d1, &ctx), resolve(&d2, &ctx)]);
        let mut rules = crate::ignore::AutoFixRules::default();
        assert!(set.auto_fixes(&rules).is_empty());
        rules.add_for_message("class 'A' lacks a base class", Some("-Wobjc-root-class"));
        let plan = set.auto_fixes(&rules);
        assert_eq!(plan.apply_to(text), "!a b\nc d\n");
    }

    #[test]
    fn positions_use_utf16_columns() {
        let text = "\u{1F600}x = 1\n";
        let ctx = DocumentContext::new(text);
        let a = resolve(&diag(Severity::Error, 1, 5, "e"), &ctx);
        let (s, e) = a.positions(&LineIndex::new(text)).unwrap();
        assert_eq!(s, TextPos { line: 0, column: 2 });
        assert_eq!(e, TextPos { line: 0, column: 3 });
    }

    #[test]
    fn map_edit_shifts_and_drops_stale_fixes() {
        let text = "aaa bbb\nccc\n";
        let ctx = DocumentContext::new(text);
        let d = diag(Severity::Error, 2, 1, "c")
            .with_fix(Fix::single("F", Location::line_col(2, 1), "Z"));
        let d2 = diag(Severity::Error, 1, 5, "b")
            .with_fix(Fix::single("F", Location::line_col(1, 5), "Q"));
        let mut set = AnnotationSet::new();
        set.replace_source("s", Origin::Document, vec![resolve(&d, &ctx), resolve(&d2, &ctx)]);
        // Insert "XX" at the start of the text.
        set.map_edit(0..0, 2);
        let v = set.visible();
        assert_eq!(v[0].range, Some(6..9));
        assert_eq!(v[1].range, Some(10..13));
        assert_eq!(v[1].fixes[0].edits[0].range, 10..10);
        // Replace "bbb" (6..9 now) with "Y": touches annotation b and its fix.
        set.map_edit(6..9, 1);
        let v = set.visible();
        assert_eq!(v[0].range, Some(6..7));
        assert!(v[0].fixes.is_empty(), "the fix inside the edited region is stale");
    }

    #[test]
    fn document_matching() {
        let doc = Path::new("/work/src/main.c");
        let ctx = DocumentContext::new("x").path(doc).alias("/tmp/zz/main.c");
        assert!(ctx.is_document(None));
        assert!(ctx.is_document(Some(Path::new("/work/src/main.c"))));
        assert!(ctx.is_document(Some(Path::new("/tmp/zz/main.c"))));
        assert!(ctx.is_document(Some(Path::new("main.c"))));
        assert!(ctx.is_document(Some(Path::new("./main.c"))));
        assert!(!ctx.is_document(Some(Path::new("util.h"))));
        assert!(!ctx.is_document(Some(Path::new("/usr/include/stdio.h"))));
        let cwd = Path::new("/work/src");
        let ctx = DocumentContext::new("x").path(doc).cwd(cwd);
        assert!(ctx.is_document(Some(Path::new("../src/main.c"))));
    }
}
