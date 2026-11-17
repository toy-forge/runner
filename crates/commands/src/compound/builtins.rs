//! Commands the interpreter implements itself on top of [`Host`] primitives: selection and
//! register manipulation, number and text tests, typed insertion, editable-mode switches.

use super::CompoundError;
use super::ast::{Arg, Call};
use super::host::{Edge, Host, KeyInput, Selection};
use super::registers::{Registers, SavedSelection};
use super::search;

/// Result of trying a builtin.
pub(super) enum Builtin {
    /// Ran (or the test passed).
    Done,
    /// A test failed, or the command could not apply.
    Fail,
    /// Needs a keystroke first.
    NeedKey,
    /// Not a builtin: hand it to the host.
    Other,
}

struct Args<'a> {
    call: &'a Call,
}

impl<'a> Args<'a> {
    fn text(&self, i: usize) -> Result<&'a str, CompoundError> {
        self.call
            .args
            .get(i)
            .and_then(Arg::as_text)
            .ok_or_else(|| CompoundError::BadArgument { command: self.call.name.clone(), index: i + 1, expected: "a string" })
    }

    fn number(&self, i: usize) -> Result<f64, CompoundError> {
        self.call
            .args
            .get(i)
            .and_then(Arg::as_number)
            .ok_or_else(|| CompoundError::BadArgument { command: self.call.name.clone(), index: i + 1, expected: "a number" })
    }
}

fn ok(b: bool) -> Builtin {
    if b { Builtin::Done } else { Builtin::Fail }
}

#[derive(Clone, Copy)]
enum Cmp {
    Lt,
    Le,
    Eq,
    Ne,
    Gt,
    Ge,
}

impl Cmp {
    fn eval(self, a: f64, b: f64) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Eq => (a - b).abs() < f64::EPSILON,
            Cmp::Ne => (a - b).abs() >= f64::EPSILON,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }
}

/// `testNumberWithName:isLessThanNumber:` and friends: the comparison and whether the second
/// operand is another named number.
fn number_test(name: &str) -> Option<(Cmp, bool)> {
    let rest = name.strip_prefix("testNumberWithName:is")?;
    for (word, cmp) in [
        ("LessThanOrEqualTo", Cmp::Le),
        ("GreaterThanOrEqualTo", Cmp::Ge),
        ("NotEqualTo", Cmp::Ne),
        ("LessThan", Cmp::Lt),
        ("GreaterThan", Cmp::Gt),
        ("EqualTo", Cmp::Eq),
    ] {
        if let Some(tail) = rest.strip_prefix(word) {
            return match tail {
                "Number:" => Some((cmp, false)),
                "NumberWithName:" | "OtherNumberName:" => Some((cmp, true)),
                _ => None,
            };
        }
    }
    None
}

fn char_len(s: &str) -> f64 {
    s.chars().count() as f64
}

fn byte_of_char(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i)
}

/// Runs `call` if it is a builtin.
pub(super) fn run<H: Host>(call: &Call, host: &mut H, regs: &mut Registers, key: Option<KeyInput>) -> Result<Builtin, CompoundError> {
    let a = Args { call };
    let name = call.name.as_str();
    let r = match name {
        // ---- no-ops and flags
        "noop" => Builtin::Done,
        "beep" => {
            host.beep();
            Builtin::Done
        }
        "debug" => {
            host.debug("debug");
            Builtin::Done
        }
        "makeEditable" => {
            host.set_editable(true);
            Builtin::Done
        }
        "makeNotEditable" => {
            host.set_editable(false);
            Builtin::Done
        }
        "testIsEditable" => ok(host.is_editable()),
        "testNotEditable" => ok(!host.is_editable()),
        "testIsSourceEditor" => ok(host.is_source_editor()),
        "testNotSourceEditor" => ok(!host.is_source_editor()),
        "testHasMultipleSelections" => ok(host.selection_count() > 1),
        "testHasSingleSelection" => ok(host.selection_count() <= 1),
        "testActiveSyntax:" => ok(host.active_syntax_matches(a.text(0)?)),
        "testActiveSyntaxNot:" => ok(!host.active_syntax_matches(a.text(0)?)),

        // ---- insertion
        "insertText:" => {
            host.insert_text(a.text(0)?);
            Builtin::Done
        }
        "insertTextAndSelect:" => {
            let start = host.selection().start;
            let t = a.text(0)?;
            host.insert_text(t);
            host.set_selection(Selection::new(start, start + t.len()));
            Builtin::Done
        }

        // ---- selection shape
        "collapseSelection" => {
            let s = host.selection();
            host.set_selection(Selection::caret(s.moving_offset()));
            Builtin::Done
        }
        "collapseSelectionToStart" => {
            let s = host.selection();
            host.set_selection(Selection::caret(s.start));
            Builtin::Done
        }
        "collapseSelectionToEnd" => {
            let s = host.selection();
            host.set_selection(Selection::caret(s.end));
            Builtin::Done
        }
        "setSelectionMovingEdgeToStart" => {
            let mut s = host.selection();
            s.edge = Edge::Start;
            host.set_selection(s);
            Builtin::Done
        }
        "setSelectionMovingEdgeToEnd" => {
            let mut s = host.selection();
            s.edge = Edge::End;
            host.set_selection(s);
            Builtin::Done
        }
        "testSelectionMovingEdgeAtStart" => ok(host.selection().edge == Edge::Start),
        "testSelectionMovingEdgeAtEnd" => ok(host.selection().edge == Edge::End),
        "testSelectedLengthEqualTo:" => ok((char_len(&host.text(host.selection().range())) - a.number(0)?).abs() < f64::EPSILON),
        "testSelectedLengthLessThan:" => ok(char_len(&host.text(host.selection().range())) < a.number(0)?),
        "testSelectedLengthGreaterThan:" => ok(char_len(&host.text(host.selection().range())) > a.number(0)?),

        // ---- named selections
        "saveSelectionWithName:" => {
            let sel = host.selection();
            let revision = host.revision();
            regs.selections.insert(a.text(0)?.to_owned(), SavedSelection { selection: sel, revision });
            Builtin::Done
        }
        "restoreSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => {
                host.set_selection(s.selection);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "swapSelectionWithName:" => {
            let n = a.text(0)?;
            let Some(saved) = regs.selections.get(n).copied() else { return Ok(Builtin::Fail) };
            let cur = SavedSelection { selection: host.selection(), revision: host.revision() };
            host.set_selection(saved.selection);
            regs.selections.insert(n.to_owned(), cur);
            Builtin::Done
        }
        "discardSelectionWithName:" => {
            regs.selections.remove(a.text(0)?);
            Builtin::Done
        }
        "renameSelectionWithName:toName:" => match regs.selections.remove(a.text(0)?) {
            Some(s) => {
                regs.selections.insert(a.text(1)?.to_owned(), s);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "selectToSelectionWithName:" | "unionSelectionWithName:" => {
            let Some(saved) = regs.selections.get(a.text(0)?).copied() else { return Ok(Builtin::Fail) };
            let cur = host.selection();
            let mut s = Selection::new(cur.start.min(saved.selection.start), cur.end.max(saved.selection.end));
            if name == "selectToSelectionWithName:" {
                s.edge = if cur.start >= saved.selection.start && cur.end >= saved.selection.end { Edge::End } else { Edge::Start };
            } else {
                s.edge = cur.edge;
            }
            host.set_selection(s);
            Builtin::Done
        }
        "testSelectionWithNameExists:" => ok(regs.selections.contains_key(a.text(0)?)),
        "testSelectionIsBeforeSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => ok(host.selection().end <= s.selection.start && host.selection() != s.selection),
            None => Builtin::Fail,
        },
        "testSelectionIsAfterSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => ok(host.selection().start >= s.selection.end && host.selection() != s.selection),
            None => Builtin::Fail,
        },
        "testSelectionIsEqualToSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => ok(host.selection().range() == s.selection.range()),
            None => Builtin::Fail,
        },
        "testSelectionNotEqualToSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => ok(host.selection().range() != s.selection.range()),
            None => Builtin::Fail,
        },
        "testDocumentEditedSinceSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => ok(host.revision() != s.revision),
            None => Builtin::Fail,
        },
        "testDocumentNotEditedSinceSelectionWithName:" => match regs.selections.get(a.text(0)?) {
            Some(s) => ok(host.revision() == s.revision),
            None => Builtin::Fail,
        },

        // ---- named texts
        "copyWithName:" => {
            let t = host.text(host.selection().range());
            regs.texts.insert(a.text(0)?.to_owned(), t);
            Builtin::Done
        }
        "cutWithName:" => {
            let t = host.text(host.selection().range());
            regs.texts.insert(a.text(0)?.to_owned(), t);
            host.insert_text("");
            Builtin::Done
        }
        "pasteWithName:" => match regs.texts.get(a.text(0)?) {
            Some(t) => {
                let t = t.clone();
                host.insert_text(&t);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "pasteAndSelectWithName:" => match regs.texts.get(a.text(0)?) {
            Some(t) => {
                let t = t.clone();
                let start = host.selection().start;
                host.insert_text(&t);
                host.set_selection(Selection::new(start, start + t.len()));
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "setText:withName:" => {
            regs.texts.insert(a.text(1)?.to_owned(), a.text(0)?.to_owned());
            Builtin::Done
        }
        "setTextWithNameFromClipboard:" => {
            regs.texts.insert(a.text(0)?.to_owned(), host.clipboard());
            Builtin::Done
        }
        "setTextWithNameToClipboard:" => match regs.texts.get(a.text(0)?) {
            Some(t) => {
                host.set_clipboard(t);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "setTextWithName:fromTextWithName:" => match regs.texts.get(a.text(1)?).cloned() {
            Some(t) => {
                regs.texts.insert(a.text(0)?.to_owned(), t);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "appendToTextWithName:text:" => {
            regs.texts.entry(a.text(0)?.to_owned()).or_default().push_str(a.text(1)?);
            Builtin::Done
        }
        "appendToTextWithName:fromTextWithName:" => match regs.texts.get(a.text(1)?).cloned() {
            Some(t) => {
                regs.texts.entry(a.text(0)?.to_owned()).or_default().push_str(&t);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "discardTextWithName:" => {
            regs.texts.remove(a.text(0)?);
            Builtin::Done
        }
        "swapCopiedTextWithName:" => {
            let n = a.text(0)?;
            let Some(t) = regs.texts.get(n).cloned() else { return Ok(Builtin::Fail) };
            let old = host.text(host.selection().range());
            let start = host.selection().start;
            host.insert_text(&t);
            host.set_selection(Selection::new(start, start + t.len()));
            regs.texts.insert(n.to_owned(), old);
            Builtin::Done
        }
        "copyTypedCharacterWithName:" => {
            let Some(k) = key else { return Ok(Builtin::NeedKey) };
            match k.character {
                Some(c) => {
                    regs.texts.insert(a.text(0)?.to_owned(), c.to_string());
                    Builtin::Done
                }
                None => Builtin::Fail,
            }
        }
        "testTextWithNameExists:" => ok(regs.texts.contains_key(a.text(0)?)),
        "testTextWithName:equals:"
        | "testTextWithName:beginsWith:"
        | "testTextWithName:endsWith:"
        | "testTextWithName:contains:"
        | "testTextWithName:containsIgnoreCase:"
        | "testTextWithName:matchesRegularExpression:" => {
            let Some(t) = regs.texts.get(a.text(0)?) else { return Ok(Builtin::Fail) };
            let v = a.text(1)?;
            ok(match name {
                "testTextWithName:equals:" => t == v,
                "testTextWithName:beginsWith:" => t.starts_with(v),
                "testTextWithName:endsWith:" => t.ends_with(v),
                "testTextWithName:contains:" => t.contains(v),
                "testTextWithName:containsIgnoreCase:" => t.to_lowercase().contains(&v.to_lowercase()),
                _ => search::compile(v)?.is_match(t).map_err(|e| CompoundError::Regex(e.to_string()))?,
            })
        }
        "replaceInTextWithName:string:replacement:" => {
            let n = a.text(0)?;
            match regs.texts.get_mut(n) {
                Some(t) => {
                    *t = t.replace(a.text(1)?, a.text(2)?);
                    Builtin::Done
                }
                None => Builtin::Fail,
            }
        }
        "replaceInTextWithName:regularExpression:replacement:" => {
            let n = a.text(0)?;
            let re = search::compile(a.text(1)?)?;
            match regs.texts.get_mut(n) {
                Some(t) => {
                    *t = re.replace_all(t, a.text(2)?).into_owned();
                    Builtin::Done
                }
                None => Builtin::Fail,
            }
        }
        "replaceInTextWithName:fromNumberWithName:toNumberWithName:replacement:" | "replaceInTextWithName:fromNumber:toNumber:replacement:" => {
            let n = a.text(0)?;
            let (from, to) = if name.contains("fromNumberWithName") {
                let (Some(f), Some(t)) = (regs.numbers.get(a.text(1)?), regs.numbers.get(a.text(2)?)) else { return Ok(Builtin::Fail) };
                (*f, *t)
            } else {
                (a.number(1)?, a.number(2)?)
            };
            let repl = a.text(3)?;
            match regs.texts.get_mut(n) {
                Some(t) => {
                    let total = t.chars().count();
                    let f = (from.max(0.0) as usize).min(total);
                    let e = (to.max(0.0) as usize).min(total).max(f);
                    let (bf, be) = (byte_of_char(t, f), byte_of_char(t, e));
                    t.replace_range(bf..be, repl);
                    Builtin::Done
                }
                None => Builtin::Fail,
            }
        }

        // ---- named numbers
        "setNumber:withName:" => {
            regs.numbers.insert(a.text(1)?.to_owned(), a.number(0)?);
            Builtin::Done
        }
        "setNumberWithName:fromNumberWithName:" => match regs.numbers.get(a.text(1)?).copied() {
            Some(v) => {
                regs.numbers.insert(a.text(0)?.to_owned(), v);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "setNumberWithName:fromTextWithName:" => match regs.texts.get(a.text(1)?).and_then(|t| t.trim().parse::<f64>().ok()) {
            Some(v) => {
                regs.numbers.insert(a.text(0)?.to_owned(), v);
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "discardNumberWithName:" => {
            regs.numbers.remove(a.text(0)?);
            Builtin::Done
        }
        "appendToNumberWithName:number:" => {
            let digit = a.number(1)?;
            let e = regs.numbers.entry(a.text(0)?.to_owned()).or_insert(0.0);
            *e = *e * 10.0 + digit;
            Builtin::Done
        }
        "testNumberWithNameExists:" => ok(regs.numbers.contains_key(a.text(0)?)),
        "addToNumberWithName:number:"
        | "addToNumberWithName:otherNumberName:"
        | "subtractFromNumberWithName:number:"
        | "subtractFromNumberWithName:otherNumberName:"
        | "multiplyNumberWithName:number:"
        | "multiplyNumberWithName:otherNumberName:"
        | "divideNumberWithName:number:"
        | "divideNumberWithName:otherNumberName:" => {
            let operand = if name.ends_with("number:") {
                a.number(1)?
            } else {
                match regs.numbers.get(a.text(1)?) {
                    Some(v) => *v,
                    None => return Ok(Builtin::Fail),
                }
            };
            let Some(cur) = regs.numbers.get_mut(a.text(0)?) else { return Ok(Builtin::Fail) };
            if name.starts_with("add") {
                *cur += operand;
            } else if name.starts_with("subtract") {
                *cur -= operand;
            } else if name.starts_with("multiply") {
                *cur *= operand;
            } else {
                if operand == 0.0 {
                    return Ok(Builtin::Fail);
                }
                *cur = (*cur / operand).trunc();
            }
            Builtin::Done
        }
        "saveSelectionStartToNumberWithName:" => {
            regs.numbers.insert(a.text(0)?.to_owned(), host.selection().start as f64);
            Builtin::Done
        }
        "saveSelectionEndToNumberWithName:" => {
            regs.numbers.insert(a.text(0)?.to_owned(), host.selection().end as f64);
            Builtin::Done
        }
        "saveSelectionLengthToNumberWithName:" => {
            regs.numbers.insert(a.text(0)?.to_owned(), char_len(&host.text(host.selection().range())));
            Builtin::Done
        }
        "saveSelectionMovingEdgeToNumberWithName:" => {
            regs.numbers.insert(a.text(0)?.to_owned(), host.selection().moving_offset() as f64);
            Builtin::Done
        }
        "setSelectionFromNumberWithName:toNumberWithName:" => match (regs.numbers.get(a.text(0)?), regs.numbers.get(a.text(1)?)) {
            (Some(f), Some(t)) => {
                let len = host.text_len();
                host.set_selection(Selection::new((*f).max(0.0) as usize, ((*t).max(0.0) as usize).min(len)));
                Builtin::Done
            }
            _ => Builtin::Fail,
        },
        "moveToLineOfNumberWithName:" => match regs.numbers.get(a.text(0)?) {
            Some(n) => {
                // 1-based line number; past the end goes to the last line.
                let line = (*n as usize).max(1) - 1;
                let at = host.offset_of_line(line).unwrap_or_else(|| {
                    let last = host.line_of_offset(host.text_len());
                    host.offset_of_line(last).unwrap_or(0)
                });
                host.set_selection(Selection::caret(at));
                Builtin::Done
            }
            None => Builtin::Fail,
        },
        "moveToColumnOfNumberWithName:" => match regs.numbers.get(a.text(0)?) {
            Some(n) => {
                // 1-based column within the current paragraph, clamped before the newline.
                let para = host.paragraph_range(host.selection().start);
                let text = host.text(para.clone());
                let line = text.strip_suffix('\n').unwrap_or(&text);
                let col = (*n as usize).max(1) - 1;
                let at = para.start + byte_of_char(line, col);
                host.set_selection(Selection::caret(at));
                Builtin::Done
            }
            None => Builtin::Fail,
        },

        _ => {
            if let Some((cmp, other_named)) = number_test(name) {
                let Some(lhs) = regs.numbers.get(a.text(0)?).copied() else { return Ok(Builtin::Fail) };
                let rhs = if other_named {
                    match regs.numbers.get(a.text(1)?) {
                        Some(v) => *v,
                        None => return Ok(Builtin::Fail),
                    }
                } else {
                    a.number(1)?
                };
                return Ok(ok(cmp.eval(lhs, rhs)));
            }
            if let Some(sn) = search::parse_name(name) {
                if sn.in_scope || sn.kind == search::Kind::Scope && sn.mode == search::Mode::Find {
                    return Ok(Builtin::Other);
                }
                return Ok(match search::run(&sn, name, &call.args, host, regs, key)? {
                    Some(b) => ok(b),
                    None => Builtin::NeedKey,
                });
            }
            Builtin::Other
        }
    };
    Ok(r)
}
