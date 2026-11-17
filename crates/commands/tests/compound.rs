//! Interpreter tests against the fake host.

use std::sync::Arc;

use scratch_commands::compound::fake::FakeHost;
use scratch_commands::compound::{CommandLibrary, CompoundCommand, CompoundError, Host, KeyInput, Machine, Registers, Selection, Status};

fn lib(cmds: &[(&str, &str)]) -> Arc<CommandLibrary> {
    let mut l = CommandLibrary::new();
    for (name, src) in cmds {
        l.add_command(name, &CompoundCommand::parse(src).expect("library command parses"));
    }
    Arc::new(l)
}

fn run_with(src: &str, host: &mut FakeHost, regs: &mut Registers, library: Arc<CommandLibrary>) -> Status {
    let cmd = CompoundCommand::parse(src).expect("test program parses");
    Machine::new(library).start_command(&cmd, None, host, regs)
}

fn run(src: &str, host: &mut FakeHost) -> Status {
    run_with(src, host, &mut Registers::new(), lib(&[]))
}

// ---- failure scopes ------------------------------------------------------------------------

#[test]
fn top_level_test_failure_restores_and_passes_through() {
    let mut h = FakeHost::new("abc");
    let s = run(r#"["insertText:", "X", "testSelectedText:", "nope"]"#, &mut h);
    assert_eq!(s, Status::PassThrough);
    assert_eq!(h.text_string(), "abc", "document restored");
}

#[test]
fn passing_tests_consume() {
    let mut h = FakeHost::new("abc");
    assert_eq!(run(r#"["testSelectedText:", "", "insertText:", "X"]"#, &mut h), Status::Consumed);
    assert_eq!(h.text_string(), "Xabc");
}

#[test]
fn failed_nested_list_is_rolled_back_but_parent_continues() {
    let mut h = FakeHost::new("abc");
    let s = run(r#"[["insertText:", "X", "testSelectedText:", "nope"], "insertText:", "Y"]"#, &mut h);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "Yabc");
}

#[test]
fn successful_nested_list_keeps_changes() {
    let mut h = FakeHost::new("abc");
    run(r#"[["insertText:", "X"], "insertText:", "Y"]"#, &mut h);
    assert_eq!(h.text_string(), "XYabc");
}

#[test]
fn break_exits_levels_successfully() {
    let mut h = FakeHost::new("");
    // break:2 leaves the inner list and the middle list; the outer list continues.
    let s = run(r#"[ [ [ "insertText:", "a", "break:", 2, "insertText:", "b" ], "insertText:", "c" ], "insertText:", "d" ]"#, &mut h);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "ad");
}

#[test]
fn break_999_exits_everything_and_consumes() {
    let mut h = FakeHost::new("");
    assert_eq!(run(r#"[ [ [ "insertText:", "a", "break:", 999 ] ], "insertText:", "z" ]"#, &mut h), Status::Consumed);
    assert_eq!(h.text_string(), "a");
}

#[test]
fn fail_exits_levels_restoring_each() {
    let mut h = FakeHost::new("");
    let s = run(r#"[ "insertText:", "x", [ [ "insertText:", "a", "fail:", 2 ], "insertText:", "c" ], "insertText:", "d" ]"#, &mut h);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "xd");
}

#[test]
fn fail_999_at_top_level_passes_through() {
    let mut h = FakeHost::new("q");
    assert_eq!(run(r#"[ "insertText:", "a", [ "fail:", 999 ] ]"#, &mut h), Status::PassThrough);
    assert_eq!(h.text_string(), "q");
}

#[test]
fn exit_consumes_without_restoring() {
    let mut h = FakeHost::new("");
    assert_eq!(run(r#"[ "insertText:", "a", [ "exit" ], "insertText:", "b" ]"#, &mut h), Status::Consumed);
    assert_eq!(h.text_string(), "a");
}

// ---- and / or / negate ---------------------------------------------------------------------

#[test]
fn negate_inverts() {
    let mut h = FakeHost::new("abc");
    assert_eq!(run(r#"["negate:", ["testHasMultipleSelections"], "insertText:", "x"]"#, &mut h), Status::Consumed);
    assert_eq!(h.text_string(), "xabc");
    let mut h = FakeHost::new("abc");
    h.selection_count = 2;
    assert_eq!(run(r#"["negate:", ["testHasMultipleSelections"], "insertText:", "x"]"#, &mut h), Status::PassThrough);
    assert_eq!(h.text_string(), "abc");
}

#[test]
fn and_passes_failure_to_parent() {
    let mut h = FakeHost::new("abc");
    let s = run(r#"["and:", ["testSelectedText:", "", "testSelectedText:", "zzz"], "insertText:", "x"]"#, &mut h);
    assert_eq!(s, Status::PassThrough);
    assert_eq!(h.text_string(), "abc");
}

#[test]
fn or_succeeds_when_any_test_passes() {
    let mut h = FakeHost::new("abc");
    let s = run(r#"["or:", ["testSelectedText:", "zzz", "testSelectedText:", ""], "insertText:", "x"]"#, &mut h);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "xabc");
}

#[test]
fn or_fails_when_all_tests_fail() {
    let mut h = FakeHost::new("abc");
    let s = run(r#"["or:", ["testSelectedText:", "q", "testSelectedText:", "r"], "insertText:", "x"]"#, &mut h);
    assert_eq!(s, Status::PassThrough);
}

#[test]
fn or_with_sublists() {
    let mut h = FakeHost::new("abc");
    let s = run(r#"["or:", [["testPrecedingText:", "z"], ["testSucceedingText:", "a"]], "insertText:", "x"]"#, &mut h);
    assert_eq!(s, Status::Consumed);
}

// ---- call / include ------------------------------------------------------------------------

#[test]
fn call_ignores_the_result_and_scopes_failures() {
    let l = lib(&[("Helper", r#"["insertText:", "H", "testSelectedText:", "never"]"#)]);
    let mut h = FakeHost::new("");
    let s = run_with(r#"["callCompoundCommand:", "Helper", "insertText:", "x"]"#, &mut h, &mut Registers::new(), l);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "x", "the helper's changes were rolled back");
}

#[test]
fn include_inlines_and_passes_failure_up() {
    let l = lib(&[("Pre", r#"["testSelectedText:", "never"]"#)]);
    let mut h = FakeHost::new("");
    let s = run_with(r#"["insertText:", "a", "includeCompoundCommand:", "Pre", "insertText:", "b"]"#, &mut h, &mut Registers::new(), l);
    assert_eq!(s, Status::PassThrough);
    assert_eq!(h.text_string(), "");
}

#[test]
fn break_inside_include_counts_the_including_list() {
    let l = lib(&[("Stop", r#"["break:", 1]"#)]);
    let mut h = FakeHost::new("");
    let s = run_with(r#"[["includeCompoundCommand:", "Stop", "insertText:", "never"], "insertText:", "after"]"#, &mut h, &mut Registers::new(), l);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "after");
}

#[test]
fn unknown_compound_is_an_error_and_restores() {
    let mut h = FakeHost::new("x");
    let s = run(r#"["insertText:", "a", "callCompoundCommand:", "Nope"]"#, &mut h);
    assert_eq!(s, Status::Error(CompoundError::UnknownCompound("Nope".into())));
    assert_eq!(h.text_string(), "x");
}

#[test]
fn unknown_host_command_is_an_error() {
    let mut h = FakeHost::new("x");
    let s = run(r#"["unknownForTests"]"#, &mut h);
    assert!(matches!(s, Status::Error(CompoundError::UnknownCommand(_))), "{s:?}");
}

#[test]
fn host_commands_run_and_unimplemented_ones_are_recorded() {
    let mut h = FakeHost::new("abc");
    run(r#"["moveRight", "moveRight", "sillyCommand"]"#, &mut h);
    assert_eq!(h.caret(), 2);
    assert_eq!(h.executed, ["sillyCommand"]);
}

// ---- recursion and repeat counts (the Vim idiom) -------------------------------------------

const REPEAT: &str = r#"[
    "testNumberWithName:isGreaterThanNumber:", "repeat", 0,
    "subtractFromNumberWithName:number:", "repeat", 1,
    ["testNumberWithName:isEqualToNumber:", "repeat", 0, "break:", 2],
    "callEnclosingCommandList:", 2,
]"#;

#[test]
fn recursive_repeat_runs_the_caller_list_count_times() {
    // Caller: move right once, then call the repeat helper which re-runs the caller list.
    let l = lib(&[("Helper: Recursive Repeat", REPEAT)]);
    let mut h = FakeHost::new("abcdefgh");
    let mut regs = Registers::new();
    regs.numbers.insert("repeat".into(), 3.0);
    let s = run_with(r#"[ ["moveRight", "callCompoundCommand:", "Helper: Recursive Repeat"] ]"#, &mut h, &mut regs, l);
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.caret(), 3, "moveRight ran three times");
    assert_eq!(regs.numbers["repeat"], 0.0);
}

#[test]
fn runaway_recursion_is_stopped() {
    let mut h = FakeHost::new("");
    let s = run(r#"[ "callEnclosingCommandList:", 1 ]"#, &mut h);
    assert_eq!(s, Status::Error(CompoundError::LimitExceeded));
}

// ---- waiting for keys ----------------------------------------------------------------------

fn start_waiting(src: &str, h: &mut FakeHost, regs: &mut Registers) -> Machine<FakeHost> {
    let cmd = CompoundCommand::parse(src).expect("parses");
    let mut m = Machine::new(lib(&[]));
    assert_eq!(m.start_command(&cmd, None, h, regs), Status::WaitingForKey);
    assert!(m.is_waiting());
    m
}

#[test]
fn object_waits_for_a_key_and_runs_the_entry() {
    let src = r#"["testSelectedText:", "", { "a": ["insertText:", "A", "break:", 2], "b": ["insertText:", "B"], "any": ["beep"] }, "insertText:", "!"]"#;
    let mut h = FakeHost::new("");
    let mut regs = Registers::new();
    let mut m = start_waiting(src, &mut h, &mut regs);
    assert_eq!(m.feed_key(KeyInput::typed('a'), &mut h, &mut regs), Status::Consumed);
    assert_eq!(h.text_string(), "A");

    let mut h = FakeHost::new("");
    let mut m = start_waiting(src, &mut h, &mut regs);
    assert_eq!(m.feed_key(KeyInput::typed('b'), &mut h, &mut regs), Status::Consumed);
    assert_eq!(h.text_string(), "B!");

    let mut h = FakeHost::new("");
    let mut m = start_waiting(src, &mut h, &mut regs);
    assert_eq!(m.feed_key(KeyInput::typed('z'), &mut h, &mut regs), Status::Consumed);
    assert_eq!(h.beeps, 1, "any matched");
}

#[test]
fn object_without_match_fails_the_list() {
    let src = r#"["insertText:", "x", { "a": ["beep"] }]"#;
    let mut h = FakeHost::new("");
    let mut regs = Registers::new();
    let mut m = start_waiting(src, &mut h, &mut regs);
    let s = m.feed_key(KeyInput::typed('q'), &mut h, &mut regs);
    // The key was consumed by the wait, so the run counts as consumed, but the document is restored.
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "");
}

#[test]
fn shifted_keys_match_by_stroke() {
    let src = r#"[{ "G": ["insertText:", "big"], "g": ["insertText:", "small"] }]"#;
    let mut h = FakeHost::new("");
    let mut regs = Registers::new();
    let mut m = start_waiting(src, &mut h, &mut regs);
    m.feed_key(KeyInput::typed('G'), &mut h, &mut regs);
    assert_eq!(h.text_string(), "big");
}

#[test]
fn typed_character_commands() {
    // Vim `f`: find the typed character on the line.
    let src = r#"["findTypedCharacterForwards"]"#;
    let mut h = FakeHost::new("hello world");
    let mut regs = Registers::new();
    let mut m = start_waiting(src, &mut h, &mut regs);
    assert_eq!(m.feed_key(KeyInput::typed('w'), &mut h, &mut regs), Status::Consumed);
    assert_eq!(h.sel(), Selection::new(6, 7));

    let src = r#"["copyTypedCharacterWithName:", "c", "findCopiedTextWithNameToEndOfLine:", "c"]"#;
    let mut h = FakeHost::new("abcabc");
    let mut m = start_waiting(src, &mut h, &mut regs);
    m.feed_key(KeyInput::typed('c'), &mut h, &mut regs);
    assert_eq!(regs.texts["c"], "c");
    assert_eq!(h.sel(), Selection::new(2, 3));
}

#[test]
fn cancel_restores_the_document() {
    let src = r#"["insertText:", "x", { "a": [] }]"#;
    let mut h = FakeHost::new("q");
    let mut regs = Registers::new();
    let mut m = start_waiting(src, &mut h, &mut regs);
    assert_eq!(h.text_string(), "xq");
    m.cancel(&mut h);
    assert_eq!(h.text_string(), "q");
    assert!(!m.is_waiting());
}

// ---- menu items ----------------------------------------------------------------------------

#[test]
fn select_menu_item_with_wildcards() {
    let mut h = FakeHost::new("").with_menu_item("Find/Find Next", true).with_menu_item("Find/Find Mode/Regular Expression", true).with_menu_item("Run/Stop", false);
    assert_eq!(run(r#"["selectMenuItem:", "Find Next"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["selectMenuItem:", "MainMenu/Find/Find Mode/Reg*"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["selectMenuItem:", "Find/Find ?ext"]"#, &mut h), Status::Consumed);
    assert_eq!(h.performed_menu_items, ["Find/Find Next", "Find/Find Mode/Regular Expression", "Find/Find Next"]);
    // Disabled and missing items fail the list.
    assert_eq!(run(r#"["selectMenuItem:", "Stop"]"#, &mut h), Status::PassThrough);
    assert_eq!(run(r#"["selectMenuItem:", "Nothing"]"#, &mut h), Status::PassThrough);
}

// ---- registers -----------------------------------------------------------------------------

#[test]
fn numbers() {
    let mut h = FakeHost::new("");
    let mut regs = Registers::new();
    let s = run_with(
        r#"["setNumber:withName:", 5, "n", "addToNumberWithName:number:", "n", 3, "multiplyNumberWithName:number:", "n", 2, "subtractFromNumberWithName:number:", "n", 1,
            "testNumberWithName:isEqualToNumber:", "n", 15, "testNumberWithName:isLessThanOrEqualToNumber:", "n", 15, "testNumberWithName:isGreaterThanNumber:", "n", 14,
            "appendToNumberWithName:number:", "digits", 4, "appendToNumberWithName:number:", "digits", 2,
            "testNumberWithName:isEqualToNumber:", "digits", 42,
            "setNumberWithName:fromNumberWithName:", "m", "n", "testNumberWithName:isEqualToNumberWithName:", "m", "n",
            "testNumberWithNameExists:", "m", "discardNumberWithName:", "m", "negate:", ["testNumberWithNameExists:", "m"]]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(s, Status::Consumed);
    assert_eq!(regs.numbers["n"], 15.0);
}

#[test]
fn missing_numbers_fail_tests() {
    let mut h = FakeHost::new("");
    assert_eq!(run(r#"["testNumberWithName:isGreaterThanNumber:", "nothing", 0]"#, &mut h), Status::PassThrough);
}

#[test]
fn named_selections() {
    let mut h = FakeHost::new("hello world");
    let mut regs = Registers::new();
    h.set_caret(0);
    let s = run_with(
        r#"["saveSelectionWithName:", "start",
            "moveRightAndModifySelection", "moveRightAndModifySelection",
            "testSelectionIsAfterSelectionWithName:", "start",
            "testSelectionWithNameExists:", "start",
            "testDocumentNotEditedSinceSelectionWithName:", "start",
            "restoreSelectionWithName:", "start",
            "testSelectionIsEqualToSelectionWithName:", "start",
            "insertText:", "X",
            "testDocumentEditedSinceSelectionWithName:", "start"]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.text_string(), "Xhello world");

    let mut h = FakeHost::new("hello world");
    h.set_caret(6);
    let s = run_with(
        r#"["saveSelectionWithName:", "w", "moveToBeginningOfDocument", "selectToSelectionWithName:", "w", "setSelectionMovingEdgeToStart"]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.sel().range(), 0..6);
    assert_eq!(h.sel().moving_offset(), 0);

    // swap / rename / discard / union
    let mut h = FakeHost::new("abcdef");
    let mut regs = Registers::new();
    h.set_caret(1);
    run_with(r#"["saveSelectionWithName:", "a", "moveRight", "moveRight", "swapSelectionWithName:", "a"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.caret(), 1);
    assert_eq!(regs.selections["a"].selection.start, 3);
    run_with(r#"["renameSelectionWithName:toName:", "a", "b", "discardSelectionWithName:", "zzz"]"#, &mut h, &mut regs, lib(&[]));
    assert!(regs.selections.contains_key("b") && !regs.selections.contains_key("a"));
    run_with(r#"["unionSelectionWithName:", "b"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.sel().range(), 1..3);
}

#[test]
fn texts() {
    let mut h = FakeHost::new("hello world").with_clipboard("clip");
    let mut regs = Registers::new();
    h.set_selection(Selection::new(0, 5));
    let s = run_with(
        r#"["copyWithName:", "w", "setText:withName:", "</", "close", "appendToTextWithName:fromTextWithName:", "close", "w", "appendToTextWithName:text:", "close", ">",
            "testTextWithName:equals:", "close", "</hello>", "testTextWithName:beginsWith:", "close", "</", "testTextWithName:endsWith:", "close", ">",
            "testTextWithName:contains:", "close", "ell", "testTextWithName:containsIgnoreCase:", "close", "ELL", "testTextWithName:matchesRegularExpression:", "close", "^</\\w+>$",
            "setTextWithNameFromClipboard:", "c", "testTextWithName:equals:", "c", "clip", "setTextWithName:fromTextWithName:", "d", "c", "testTextWithNameExists:", "d",
            "discardTextWithName:", "d", "negate:", ["testTextWithNameExists:", "d"],
            "pasteWithName:", "close"]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(s, Status::Consumed, "{regs:?}");
    assert_eq!(h.text_string(), "</hello> world");
}

#[test]
fn cut_paste_select_and_swap() {
    let mut h = FakeHost::new("abc def");
    let mut regs = Registers::new();
    h.set_selection(Selection::new(0, 3));
    run_with(r#"["cutWithName:", "x"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.text_string(), " def");
    run_with(r#"["moveToEndOfDocument", "pasteAndSelectWithName:", "x"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.text_string(), " defabc");
    assert_eq!(h.sel().range(), 4..7);
    run_with(r#"["setText:withName:", "ZZ", "y", "swapCopiedTextWithName:", "y"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.text_string(), " defZZ");
    assert_eq!(regs.texts["y"], "abc");
}

#[test]
fn replace_in_text_register() {
    let mut h = FakeHost::new("");
    let mut regs = Registers::new();
    let s = run_with(
        r#"["setText:withName:", "a-b-c", "t", "replaceInTextWithName:string:replacement:", "t", "-", "+", "testTextWithName:equals:", "t", "a+b+c",
            "replaceInTextWithName:regularExpression:replacement:", "t", "\\w(?=\\+)", "X", "testTextWithName:equals:", "t", "X+X+c",
            "replaceInTextWithName:fromNumber:toNumber:replacement:", "t", 1, 4, "_", "testTextWithName:equals:", "t", "X_c"]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(s, Status::Consumed, "{regs:?}");
}

#[test]
fn selection_numbers_and_navigation() {
    let mut h = FakeHost::new("one\ntwo\nthree");
    let mut regs = Registers::new();
    let s = run_with(
        r#"["setNumber:withName:", 2, "line", "moveToLineOfNumberWithName:", "line", "setNumber:withName:", 3, "col", "moveToColumnOfNumberWithName:", "col",
            "saveSelectionStartToNumberWithName:", "pos"]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(s, Status::Consumed);
    assert_eq!(regs.numbers["pos"], 6.0, "line 2, column 3");
    run_with(r#"["setNumber:withName:", 99, "line", "moveToLineOfNumberWithName:", "line"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.caret(), 8, "past the end goes to the last line");
    h.set_selection(Selection::new(1, 3));
    run_with(
        r#"["saveSelectionLengthToNumberWithName:", "len", "saveSelectionEndToNumberWithName:", "end", "saveSelectionMovingEdgeToNumberWithName:", "edge",
            "testSelectedLengthEqualTo:", 2, "testSelectedLengthGreaterThan:", 1, "testSelectedLengthLessThan:", 3]"#,
        &mut h,
        &mut regs,
        lib(&[]),
    );
    assert_eq!(regs.numbers["len"], 2.0);
    assert_eq!(regs.numbers["end"], 3.0);
    run_with(r#"["setNumber:withName:", 0, "a", "setNumber:withName:", 4, "b", "setSelectionFromNumberWithName:toNumberWithName:", "a", "b"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(h.sel().range(), 0..4);
}

// ---- editable mode and misc tests ----------------------------------------------------------

#[test]
fn editable_mode_switches() {
    let mut h = FakeHost::new("abc");
    assert_eq!(run(r#"["makeNotEditable", "testNotEditable", "negate:", ["testIsEditable"]]"#, &mut h), Status::Consumed);
    assert!(!h.is_editable());
    assert_eq!(run(r#"["testIsEditable"]"#, &mut h), Status::PassThrough);
    assert_eq!(run(r#"["makeEditable", "testIsEditable", "testIsSourceEditor", "testHasSingleSelection"]"#, &mut h), Status::Consumed);
}

#[test]
fn active_syntax_and_scope_tests() {
    let mut h = FakeHost::new("let x = \"a\";");
    h.scopes.push(("source".into(), 0..4));
    h.scopes.push(("string".into(), 8..11));
    h.set_caret(2);
    assert_eq!(run(r#"["testActiveSyntax:", "source"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testActiveSyntaxNot:", "source"]"#, &mut h), Status::PassThrough);
    h.set_caret(9);
    assert_eq!(run(r#"["testActiveSyntax:", "source"]"#, &mut h), Status::PassThrough);
    assert_eq!(run(r#"["testSelectedScope:", "string"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testPrecedingScope:", "string"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSelectedScopeNot:", "string"]"#, &mut h), Status::PassThrough);
}

#[test]
fn insert_text_and_select() {
    let mut h = FakeHost::new("ab");
    h.set_caret(1);
    run(r#"["insertTextAndSelect:", "XY"]"#, &mut h);
    assert_eq!(h.text_string(), "aXYb");
    assert_eq!(h.sel().range(), 1..3);
}

#[test]
fn collapse_and_moving_edge() {
    let mut h = FakeHost::new("abcdef");
    h.set_selection(Selection::new(1, 4));
    run(r#"["setSelectionMovingEdgeToStart", "testSelectionMovingEdgeAtStart", "collapseSelection"]"#, &mut h);
    assert_eq!(h.sel(), Selection::caret(1));
    h.set_selection(Selection::new(1, 4));
    run(r#"["collapseSelectionToEnd"]"#, &mut h);
    assert_eq!(h.caret(), 4);
    h.set_selection(Selection::new(1, 4));
    run(r#"["collapseSelectionToStart"]"#, &mut h);
    assert_eq!(h.caret(), 1);
    h.set_selection(Selection::new(1, 4));
    assert_eq!(run(r#"["testSelectionMovingEdgeAtStart"]"#, &mut h), Status::PassThrough);
}

// ---- text tests and finds ------------------------------------------------------------------

#[test]
fn text_tests() {
    let mut h = FakeHost::new("foo(bar)");
    h.set_selection(Selection::new(4, 7));
    assert_eq!(run(r#"["testSelectedText:", "bar", "testPrecedingText:", "foo(", "testSucceedingText:", ")"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSelectedTextNot:", "baz", "testPrecedingTextNot:", "x", "testSucceedingTextNot:", "x"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSelectedTextIgnoreCase:", "BAR"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSelectedText:", "BAR"]"#, &mut h), Status::PassThrough);
    assert_eq!(run(r#"["testPrecedingTextIgnoreCase:", "FOO("]"#, &mut h), Status::Consumed);
}

#[test]
fn regex_tests() {
    let mut h = FakeHost::new("x = <div class=a>");
    h.set_selection(Selection::new(4, 17));
    assert_eq!(run(r#"["testSelectedRegularExpression:", "<(\\w+)[^>]*(?<!/)>"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSelectedRegularExpression:", "<\\w+"]"#, &mut h), Status::PassThrough, "must match the whole selection");
    h.set_caret(4);
    assert_eq!(run(r#"["testPrecedingRegularExpression:", "=\\s"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testPrecedingRegularExpression:", ";|\\s|^|$"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSucceedingRegularExpression:", "<\\w+"]"#, &mut h), Status::Consumed);
    assert_eq!(run(r#"["testSucceedingRegularExpressionNot:", "z"]"#, &mut h), Status::Consumed);
    let s = run(r#"["testSelectedRegularExpression:", "("]"#, &mut h);
    assert!(matches!(s, Status::Error(CompoundError::Regex(_))), "{s:?}");
}

#[test]
fn preceding_regex_with_line_start_alternative() {
    // Shell `[` rule: after whitespace, `;`, `&`, line start...
    let mut h = FakeHost::new("echo ok\n");
    h.set_caret(8);
    assert_eq!(run(r#"["testPrecedingRegularExpression:", ";|&|\\[|\\s|^|$"]"#, &mut h), Status::Consumed);
    h.set_caret(6);
    assert_eq!(run(r#"["testPrecedingRegularExpression:", ";|&|\\[|\\s"]"#, &mut h), Status::PassThrough);
}

#[test]
fn copied_text_tests() {
    let mut h = FakeHost::new("</a>x");
    let mut regs = Registers::new();
    let s = run_with(r#"["setText:withName:", "</a>", "close", "testSucceedingTextIsCopiedTextWithName:", "close", "testSucceedingTextNotCopiedTextWithName:", "other"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(s, Status::Consumed);
    let s = run_with(r#"["testSucceedingTextNotCopiedTextWithName:", "close"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(s, Status::PassThrough);
}

#[test]
fn finds() {
    let mut h = FakeHost::new("one two one\nthree one");
    h.set_caret(0);
    assert_eq!(run(r#"["findText:", "one"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 0..3, "a caret at 0 finds the `one` that starts there");
}

#[test]
fn find_forwards_starts_after_selection() {
    let mut h = FakeHost::new("one two one\nthree one");
    h.set_selection(Selection::new(0, 3));
    assert_eq!(run(r#"["findTextForwards:", "one"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 8..11);
    assert_eq!(h.sel().edge, scratch_commands::compound::Edge::End);
}

#[test]
fn find_backwards_and_anchored() {
    let mut h = sample();
    h.set_selection(Selection::new(8, 11));
    assert_eq!(run(r#"["findTextBackwards:", "one"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 0..3);
    assert_eq!(h.sel().edge, scratch_commands::compound::Edge::Start);

    let mut h = FakeHost::new("ab cd");
    h.set_selection(Selection::new(0, 2));
    assert_eq!(run(r#"["findTextForwardsAnchored:", "cd"]"#, &mut h), Status::PassThrough);
    assert_eq!(run(r#"["findTextForwardsAnchored:", " c"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 2..4);
    h.set_selection(Selection::new(3, 5));
    assert_eq!(run(r#"["findTextBackwardsAnchored:", " "]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 2..3);
}

fn sample() -> FakeHost {
    FakeHost::new("one two one\nthree one")
}

#[test]
fn find_to_end_of_line_and_locally() {
    let mut h = FakeHost::new("a b\nc d");
    h.set_caret(0);
    assert_eq!(run(r#"["findTextToEndOfLine:", "d"]"#, &mut h), Status::PassThrough, "d is on the next line");
    assert_eq!(run(r#"["findTextToEndOfLine:", "b"]"#, &mut h), Status::Consumed);
    h.set_caret(0);
    assert_eq!(run(r#"["findTextLocallyForwards:", "d"]"#, &mut h), Status::PassThrough);
    h.set_caret(7);
    assert_eq!(run(r#"["findTextToBeginningOfLine:", "c"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 4..5);
}

#[test]
fn find_regex_ignore_case_and_in_selection() {
    let mut h = FakeHost::new("Foo foo FOO");
    h.set_caret(0);
    assert_eq!(run(r#"["findRegularExpressionForwards:", "f\\w+"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 4..7);
    h.set_caret(0);
    assert_eq!(run(r#"["findTextIgnoreCaseForwards:", "FOO"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 0..3);
    h.set_caret(0);
    h.set_selection(Selection::new(4, 11));
    assert_eq!(run(r#"["findTextInSelection:", "FOO"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 8..11);
    h.set_selection(Selection::new(0, 11));
    assert_eq!(run(r#"["findTextInSelectionBackwards:", "foo"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 4..7);
}

#[test]
fn find_to_named_selection() {
    let mut h = FakeHost::new("a x b x c x");
    let mut regs = Registers::new();
    h.set_selection(Selection::new(6, 7));
    run_with(r#"["saveSelectionWithName:", "limit"]"#, &mut h, &mut regs, lib(&[]));
    h.set_caret(0);
    let s = run_with(r#"["findText:toSelectionWithName:", "x", "limit"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(s, Status::Consumed);
    assert_eq!(h.sel().range(), 2..3);
    h.set_caret(7);
    let s = run_with(r#"["findText:toSelectionWithName:", "x", "limit"]"#, &mut h, &mut regs, lib(&[]));
    assert_eq!(s, Status::PassThrough, "the only later x is beyond the limit selection");
}

// ---- Entry points and libraries -------------------------------------------------------------

#[test]
fn dictionary_entries_run_by_key() {
    let cmd = CompoundCommand::parse(r#"{ "{": ["insertText:", "{}", "moveLeft"], "\r": ["insertText:", "\n"] }"#).expect("parses");
    let mut h = FakeHost::new("");
    let mut regs = Registers::new();
    let mut m = Machine::new(lib(&[]));
    assert_eq!(m.start_command(&cmd, Some("{"), &mut h, &mut regs), Status::Consumed);
    assert_eq!(h.text_string(), "{}");
    assert_eq!(h.caret(), 1);
    assert_eq!(m.start_command(&cmd, Some("q"), &mut h, &mut regs), Status::PassThrough);
}

#[test]
fn library_registers_dictionary_entries_by_name() {
    let cmd = CompoundCommand::parse(r#"{ "Named": ["insertText:", "N"] }"#).expect("parses");
    let mut l = CommandLibrary::new();
    l.add_command("ignored title", &cmd);
    assert!(l.get("Named").is_some());
    assert!(l.get("ignored title").is_none());
    assert_eq!(l.len(), 1);
    assert!(!l.is_empty());
}

#[test]
fn balanced_delimiters_via_host() {
    let mut h = FakeHost::new("f(a, b)");
    h.set_caret(3);
    assert_eq!(run(r#"["balanceDelimiters"]"#, &mut h), Status::Consumed);
    assert_eq!(h.sel().range(), 1..7);
}

#[test]
fn spec_example_from_the_help_book() {
    // ["moveLeft", "insertText:", "hello", "surroundSelectionWithString:endString:", "(", ")"]
    let mut h = FakeHost::new("ab");
    h.set_caret(2);
    run(r#"["moveLeft", "insertText:", "hello", "surroundSelectionWithString:endString:", "(", ")"]"#, &mut h);
    assert_eq!(h.text_string(), "ahellob");
    assert_eq!(h.executed, ["surroundSelectionWithString:endString:"]);
}
