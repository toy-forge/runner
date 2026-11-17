//! The `test*` / `find*` text command family: name grammar and implementation.
//!
//! Names are `(test|find)(Selected|Preceding|Succeeding)?` followed by a kind
//! (`Text`, `TextIgnoreCase`, `RegularExpression`, `CopiedTextWithName`, `TypedCharacter`,
//! `Scope`) and modifiers (`Not`, `Is`, `Forwards`, `Backwards`, `ForwardsAnchored`, `ToEndOfLine`,
//! `InSelection`, `Locally...`, `ToSelectionWithName` ...).

use std::ops::Range;

use fancy_regex::Regex;

use super::CompoundError;
use super::ast::Arg;
use super::host::{Edge, Host, KeyInput, Selection};
use super::registers::Registers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Test,
    Find,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Where {
    Selected,
    Preceding,
    Succeeding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Text,
    TextIgnoreCase,
    Regex,
    CopiedText,
    TypedCharacter,
    Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Forwards,
    Backwards,
    ForwardsAnchored,
    BackwardsAnchored,
    ToEndOfLine,
    ToBeginningOfLine,
    InSelection,
    InSelectionBackwards,
}

/// A parsed search command name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchName {
    pub mode: Mode,
    pub whr: Option<Where>,
    pub kind: Kind,
    pub dir: Dir,
    pub not: bool,
    pub locally: bool,
    /// The command has a `ToSelectionWithName:` argument.
    pub to_selection: bool,
    /// The command has an `InScope:` argument.
    pub in_scope: bool,
    /// Number of arguments the name declares (colons).
    pub arity: usize,
}

const TOKENS: &[&str] = &[
    "TextIgnoreCase",
    "Text",
    "RegularExpression",
    "CopiedTextWithName",
    "TypedCharacter",
    "Scope",
    "Not",
    "Is",
    "ForwardsAnchored",
    "BackwardsAnchored",
    "Forwards",
    "Backwards",
    "ToBeginningOfDocument",
    "ToBeginningOfLine",
    "ToEndOfLine",
    "ToEndOfDocument",
    "InSelectionBackwards",
    "InSelection",
    "LocallyToSelectionWithName",
    "ToSelectionWithName",
    "Locally",
    "InScope",
];

pub(super) fn parse_name(name: &str) -> Option<SearchName> {
    let arity = name.matches(':').count();
    // `findText:toSelectionWithName:inScope:` splits into `findText` `toSelectionWithName` `inScope`:
    // re-capitalise the parts after the first colon so they tokenise like the compact names.
    let mut compact = String::new();
    for (i, part) in name.split(':').enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            compact.push_str(part);
        } else {
            let mut cs = part.chars();
            if let Some(f) = cs.next() {
                compact.extend(f.to_uppercase());
                compact.push_str(cs.as_str());
            }
        }
    }
    let (mode, rest) = compact
        .strip_prefix("test")
        .map(|r| (Mode::Test, r))
        .or_else(|| compact.strip_prefix("find").map(|r| (Mode::Find, r)))?;
    let (whr, mut rest) = if let Some(r) = rest.strip_prefix("Selected") {
        (Some(Where::Selected), r)
    } else if let Some(r) = rest.strip_prefix("Preceding") {
        (Some(Where::Preceding), r)
    } else if let Some(r) = rest.strip_prefix("Succeeding") {
        (Some(Where::Succeeding), r)
    } else {
        (None, rest)
    };
    let mut kind = None;
    let mut dir = Dir::Forwards;
    let mut not = false;
    let mut locally = false;
    let mut to_selection = false;
    let mut in_scope = false;
    'outer: while !rest.is_empty() {
        for t in TOKENS {
            if let Some(r) = rest.strip_prefix(t) {
                match *t {
                    "TextIgnoreCase" => kind = Some(Kind::TextIgnoreCase),
                    "Text" => {
                        if kind != Some(Kind::CopiedText) {
                            kind = Some(Kind::Text)
                        }
                    }
                    "RegularExpression" => kind = Some(Kind::Regex),
                    "CopiedTextWithName" => kind = Some(Kind::CopiedText),
                    "TypedCharacter" => kind = Some(Kind::TypedCharacter),
                    "Scope" => kind = Some(Kind::Scope),
                    "Not" => not = true,
                    "Is" => {}
                    "ForwardsAnchored" => dir = Dir::ForwardsAnchored,
                    "BackwardsAnchored" => dir = Dir::BackwardsAnchored,
                    "Forwards" | "ToEndOfDocument" => dir = Dir::Forwards,
                    "Backwards" | "ToBeginningOfDocument" => dir = Dir::Backwards,
                    "ToBeginningOfLine" => dir = Dir::ToBeginningOfLine,
                    "ToEndOfLine" => dir = Dir::ToEndOfLine,
                    "InSelectionBackwards" => dir = Dir::InSelectionBackwards,
                    "InSelection" => dir = Dir::InSelection,
                    "LocallyToSelectionWithName" => {
                        locally = true;
                        to_selection = true
                    }
                    "ToSelectionWithName" => to_selection = true,
                    "Locally" => locally = true,
                    "InScope" => in_scope = true,
                    _ => {}
                }
                rest = r;
                continue 'outer;
            }
        }
        return None;
    }
    Some(SearchName { mode, whr, kind: kind?, dir, not, locally, to_selection, in_scope, arity })
}

// ---- matching helpers -----------------------------------------------------------------------

pub(super) fn compile(pattern: &str) -> Result<Regex, CompoundError> {
    Regex::new(&format!("(?m){pattern}")).map_err(|e| CompoundError::Regex(e.to_string()))
}

fn eq_ic(a: &str, b: &str, ic: bool) -> bool {
    if ic { a.to_lowercase() == b.to_lowercase() } else { a == b }
}

fn ends_with(hay: &str, needle: &str, ic: bool) -> bool {
    if ic { hay.to_lowercase().ends_with(&needle.to_lowercase()) } else { hay.ends_with(needle) }
}

fn starts_with(hay: &str, needle: &str, ic: bool) -> bool {
    if ic { hay.to_lowercase().starts_with(&needle.to_lowercase()) } else { hay.starts_with(needle) }
}

/// First or last occurrence of `pat` in `text` as byte range.
fn find_in(text: &str, pat: &Pattern, backwards: bool) -> Result<Option<Range<usize>>, CompoundError> {
    match pat {
        Pattern::Literal { text: needle, ignore_case } => {
            if needle.is_empty() {
                return Ok(None);
            }
            if *ignore_case {
                let hay = text.to_lowercase();
                let n = needle.to_lowercase();
                // Only exact when lowercasing keeps byte lengths (true for ASCII and most text).
                if hay.len() != text.len() {
                    return Ok(None);
                }
                let at = if backwards { hay.rfind(&n) } else { hay.find(&n) };
                Ok(at.map(|i| i..i + n.len()))
            } else {
                let at = if backwards { text.rfind(needle.as_str()) } else { text.find(needle.as_str()) };
                Ok(at.map(|i| i..i + needle.len()))
            }
        }
        Pattern::Regex(re) => {
            let mut found = None;
            for m in re.find_iter(text) {
                let m = m.map_err(|e| CompoundError::Regex(e.to_string()))?;
                found = Some(m.start()..m.end());
                if !backwards {
                    break;
                }
            }
            Ok(found)
        }
    }
}

enum Pattern {
    Literal { text: String, ignore_case: bool },
    Regex(Regex),
}

fn arg_text<'a>(args: &'a [Arg], i: usize, name: &str) -> Result<&'a str, CompoundError> {
    args.get(i).and_then(Arg::as_text).ok_or_else(|| CompoundError::BadArgument { command: name.to_owned(), index: i + 1, expected: "a string" })
}

/// Runs a parsed search command. Returns `Ok(Some(true/false))` for the test result, or
/// `Ok(None)` when a key is needed first.
pub(super) fn run<H: Host>(
    sn: &SearchName,
    name: &str,
    args: &[Arg],
    host: &mut H,
    regs: &mut Registers,
    key: Option<KeyInput>,
) -> Result<Option<bool>, CompoundError> {
    // Resolve the pattern.
    let mut next_arg = 0;
    let pattern = match sn.kind {
        Kind::Scope => {
            let selector = arg_text(args, 0, name)?;
            let sel = host.selection();
            let at = match sn.whr.unwrap_or(Where::Selected) {
                Where::Selected => sel.start,
                Where::Preceding => sel.start.saturating_sub(1),
                Where::Succeeding => sel.end,
            };
            return Ok(Some(host.scope_matches(selector, at) != sn.not));
        }
        Kind::TypedCharacter => {
            let Some(k) = key else { return Ok(None) };
            let Some(c) = k.character else { return Ok(Some(false)) };
            Pattern::Literal { text: c.to_string(), ignore_case: false }
        }
        Kind::CopiedText => {
            let n = arg_text(args, next_arg, name)?;
            next_arg += 1;
            match regs.texts.get(n) {
                Some(t) => Pattern::Literal { text: t.clone(), ignore_case: false },
                None => return Ok(Some(sn.not)),
            }
        }
        Kind::Text | Kind::TextIgnoreCase => {
            let t = arg_text(args, next_arg, name)?;
            next_arg += 1;
            Pattern::Literal { text: t.to_owned(), ignore_case: sn.kind == Kind::TextIgnoreCase }
        }
        Kind::Regex => {
            let t = arg_text(args, next_arg, name)?;
            next_arg += 1;
            Pattern::Regex(compile(t)?)
        }
    };
    let sel = host.selection();
    match sn.mode {
        Mode::Test => {
            let whr = sn.whr.unwrap_or(Where::Selected);
            let ok = test(&pattern, whr, host, sel)?;
            Ok(Some(ok != sn.not))
        }
        Mode::Find => {
            let bound = if sn.to_selection {
                let n = arg_text(args, next_arg, name)?;
                match regs.selections.get(n) {
                    Some(s) => Some(s.selection),
                    None => return Ok(Some(false)),
                }
            } else {
                None
            };
            let r = find(&pattern, sn, host, sel, bound)?;
            match r {
                Some(range) => {
                    let mut s = Selection::new(range.start, range.end);
                    s.edge = if matches!(sn.dir, Dir::Backwards | Dir::BackwardsAnchored | Dir::ToBeginningOfLine | Dir::InSelectionBackwards) { Edge::Start } else { Edge::End };
                    host.set_selection(s);
                    Ok(Some(!sn.not))
                }
                None => Ok(Some(sn.not)),
            }
        }
    }
}

fn test<H: Host>(pat: &Pattern, whr: Where, host: &H, sel: Selection) -> Result<bool, CompoundError> {
    match (pat, whr) {
        (Pattern::Literal { text, ignore_case }, Where::Selected) => Ok(eq_ic(&host.text(sel.range()), text, *ignore_case)),
        (Pattern::Literal { text, ignore_case }, Where::Preceding) => {
            let from = if text.contains('\n') { 0 } else { host.paragraph_range(sel.start).start };
            let before = host.text(from..sel.start);
            Ok(ends_with(&before, text, *ignore_case))
        }
        (Pattern::Literal { text, ignore_case }, Where::Succeeding) => {
            let to = if text.contains('\n') { host.text_len() } else { host.paragraph_range(sel.end).end.max(sel.end) };
            let after = host.text(sel.end..to);
            Ok(starts_with(&after, text, *ignore_case))
        }
        (Pattern::Regex(re), Where::Selected) => {
            let t = host.text(sel.range());
            match re.find(&t).map_err(|e| CompoundError::Regex(e.to_string()))? {
                Some(m) => Ok(m.start() == 0 && m.end() == t.len()),
                None => Ok(false),
            }
        }
        (Pattern::Regex(re), Where::Preceding) => {
            let para = host.paragraph_range(sel.start);
            let before = host.text(para.start..sel.start);
            for m in re.find_iter(&before) {
                let m = m.map_err(|e| CompoundError::Regex(e.to_string()))?;
                if m.end() == before.len() {
                    return Ok(true);
                }
            }
            // A pattern that can match empty text matches at the end of the prefix too.
            Ok(re.find_from_pos(&before, before.len()).map_err(|e| CompoundError::Regex(e.to_string()))?.is_some_and(|m| m.start() == before.len()))
        }
        (Pattern::Regex(re), Where::Succeeding) => {
            let para = host.paragraph_range(sel.end);
            let after = host.text(sel.end..para.end.max(sel.end));
            Ok(re.find(&after).map_err(|e| CompoundError::Regex(e.to_string()))?.is_some_and(|m| m.start() == 0))
        }
    }
}

fn find<H: Host>(pat: &Pattern, sn: &SearchName, host: &H, sel: Selection, bound: Option<Selection>) -> Result<Option<Range<usize>>, CompoundError> {
    let len = host.text_len();
    let para = |at| host.paragraph_range(at);
    // Search region and direction.
    let (region, backwards): (Range<usize>, bool) = match sn.dir {
        Dir::Forwards | Dir::ForwardsAnchored => (sel.end..len, false),
        Dir::Backwards | Dir::BackwardsAnchored => (0..sel.start, true),
        Dir::ToEndOfLine => (sel.end..para(sel.end).end.max(sel.end), false),
        Dir::ToBeginningOfLine => (para(sel.start).start..sel.start, true),
        Dir::InSelection => (sel.range(), false),
        Dir::InSelectionBackwards => (sel.range(), true),
    };
    let mut region = region;
    if sn.locally {
        let p = para(if backwards { sel.start } else { sel.end });
        region = region.start.max(p.start)..region.end.min(p.end);
    }
    if let Some(b) = bound {
        if backwards {
            region = region.start.max(b.end.min(sel.start))..region.end;
        } else {
            region = region.start..region.end.min(b.start.max(sel.end));
        }
    }
    if region.start > region.end {
        return Ok(None);
    }
    let text = host.text(region.clone());
    let found = match sn.dir {
        Dir::ForwardsAnchored => find_in(&text, pat, false)?.filter(|m| m.start == 0),
        Dir::BackwardsAnchored => find_in(&text, pat, true)?.filter(|m| m.end == text.len()),
        _ => find_in(&text, pat, backwards)?,
    };
    Ok(found.map(|m| region.start + m.start..region.start + m.end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        let n = parse_name("testPrecedingText:").unwrap();
        assert_eq!((n.mode, n.whr, n.kind, n.not), (Mode::Test, Some(Where::Preceding), Kind::Text, false));
        let n = parse_name("testSucceedingTextNot:").unwrap();
        assert!(n.not);
        let n = parse_name("testSelectedRegularExpression:").unwrap();
        assert_eq!(n.kind, Kind::Regex);
        let n = parse_name("testSucceedingTextNotCopiedTextWithName:").unwrap();
        assert_eq!((n.kind, n.not), (Kind::CopiedText, true));
        let n = parse_name("testSucceedingTextIsCopiedTextWithName:").unwrap();
        assert_eq!((n.kind, n.not), (Kind::CopiedText, false));
        let n = parse_name("findRegularExpressionToEndOfLine:").unwrap();
        assert_eq!((n.mode, n.kind, n.dir), (Mode::Find, Kind::Regex, Dir::ToEndOfLine));
        let n = parse_name("findTextLocallyBackwards:").unwrap();
        assert!(n.locally);
        assert_eq!(n.dir, Dir::Backwards);
        let n = parse_name("findScopeForwardsAnchored:").unwrap();
        assert_eq!((n.kind, n.dir), (Kind::Scope, Dir::ForwardsAnchored));
        let n = parse_name("findText:toSelectionWithName:inScope:").unwrap();
        assert!(n.to_selection && n.in_scope);
        assert_eq!(n.arity, 3);
        let n = parse_name("findTypedCharacterBackwards").unwrap();
        assert_eq!((n.kind, n.dir, n.arity), (Kind::TypedCharacter, Dir::Backwards, 0));
        let n = parse_name("findCopiedTextWithNameToEndOfLine:").unwrap();
        assert_eq!((n.kind, n.dir), (Kind::CopiedText, Dir::ToEndOfLine));
        assert!(parse_name("testIsEditable").is_none());
        assert!(parse_name("testHasMultipleSelections").is_none());
        assert!(parse_name("testNumberWithName:isLessThanNumber:").is_none());
        assert!(parse_name("moveLeft").is_none());
    }
}
