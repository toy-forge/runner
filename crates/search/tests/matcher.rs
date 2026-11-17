use scratch_search::*;

fn m(p: &str, mode: MatchMode, ic: bool) -> Matcher {
    Matcher::simple(p, mode, ic).unwrap()
}

fn all(mt: &Matcher, text: &str) -> Vec<String> {
    mt.find_all(text)
        .unwrap()
        .into_iter()
        .map(|r| text[r].to_string())
        .collect()
}

#[test]
fn text_mode_and_ignore_case() {
    let t = "Foo foo FOO fo.o";
    assert_eq!(
        all(&m("foo", MatchMode::Text, true), t),
        ["Foo", "foo", "FOO"]
    );
    assert_eq!(all(&m("foo", MatchMode::Text, false), t), ["foo"]);
    assert_eq!(all(&m("fo.o", MatchMode::Text, true), t), ["fo.o"]);
    assert!(m("", MatchMode::Text, true).find_all(t).unwrap().is_empty());
}

#[test]
fn word_modes() {
    let t = "cat concat catalog cat_ cat.";
    assert_eq!(
        m("cat", MatchMode::ExactWord, false).find_all(t).unwrap(),
        vec![0..3, 24..27]
    );
    assert_eq!(
        m("cat", MatchMode::WordStartsWith, false)
            .find_all(t)
            .unwrap(),
        vec![0..3, 11..14, 19..22, 24..27]
    );
    assert_eq!(
        m("cat", MatchMode::WordEndsWith, false)
            .find_all(t)
            .unwrap(),
        vec![0..3, 7..10, 24..27]
    );
    // non-word edges do not require a boundary
    assert_eq!(
        m("cat.", MatchMode::ExactWord, false).find_all(t).unwrap(),
        vec![24..28]
    );
}

#[test]
fn ignoring_whitespace() {
    let t = "foo  bar foobar foo\n\tbar";
    assert_eq!(
        m("foo bar", MatchMode::IgnoringWhitespace, true)
            .find_all(t)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn regex_oniguruma_features() {
    let t = "price: 10 USD, cost: 20 EUR";
    let lb = m(r"(?<=: )\d+", MatchMode::Regex, true);
    assert_eq!(all(&lb, t), ["10", "20"]);
    let back = m(r"(\w)\1", MatchMode::Regex, true);
    assert_eq!(all(&back, "aabbcd"), ["aa", "bb"]);
    let named = m(r"(?<n>\d+) (?<c>[A-Z]+)", MatchMode::Regex, true);
    assert_eq!(named.group_count(), 2);
    assert_eq!(named.group_names(), ["n", "c"]);
    assert_eq!(
        all(&m(r"\h+", MatchMode::Regex, true), "xyz 1aF zz"),
        ["1aF"]
    );
    // ^ and $ are line anchors
    assert_eq!(
        all(&m(r"^\w+$", MatchMode::Regex, true), "a\nbc\nd e"),
        ["a", "bc"]
    );
    // Ruby (?m): dot matches newline
    assert_eq!(
        all(&m(r"(?m)a.b", MatchMode::Regex, true), "a\nb"),
        ["a\nb"]
    );
    assert!(all(&m(r"a.b", MatchMode::Regex, true), "a\nb").is_empty());
}

#[test]
fn invalid_regex_is_error() {
    assert!(matches!(
        Matcher::simple("(", MatchMode::Regex, true),
        Err(MatchError::InvalidRegex(_))
    ));
    assert!(matches!(
        Matcher::simple(r"[a-", MatchMode::Regex, true),
        Err(MatchError::InvalidRegex(_))
    ));
    // Text mode never fails.
    assert!(Matcher::simple("(", MatchMode::Text, true).is_ok());
}

#[test]
fn backtrack_limit_is_reported() {
    let opts = MatcherOptions {
        mode: MatchMode::Regex,
        backtrack_limit: 1000,
        ..Default::default()
    };
    let mt = Matcher::new(r"^(a*)*\1b", &opts).unwrap();
    let text = "a".repeat(40);
    assert_eq!(mt.find_all(&text), Err(MatchError::BacktrackLimit));
}

#[test]
fn find_next_prev_wrap() {
    let t = "ab ab ab";
    let mt = m("ab", MatchMode::Text, true);
    assert_eq!(
        mt.find_next(t, 0, true).unwrap(),
        Some(Found {
            range: 0..2,
            wrapped: false
        })
    );
    assert_eq!(mt.find_next(t, 2, true).unwrap().unwrap().range, 3..5);
    assert_eq!(mt.find_next(t, 7, false).unwrap(), None);
    assert_eq!(
        mt.find_next(t, 7, true).unwrap(),
        Some(Found {
            range: 0..2,
            wrapped: true
        })
    );
    assert_eq!(
        mt.find_prev(t, 6, true).unwrap(),
        Some(Found {
            range: 3..5,
            wrapped: false
        })
    );
    assert_eq!(mt.find_prev(t, 0, false).unwrap(), None);
    assert_eq!(
        mt.find_prev(t, 0, true).unwrap(),
        Some(Found {
            range: 6..8,
            wrapped: true
        })
    );
    assert_eq!(mt.find_prev(t, 1, false).unwrap().unwrap().range, 0..2);
    assert_eq!(mt.find_next("", 0, true).unwrap(), None);
}

#[test]
fn find_prev_in_huge_text_scans_windows() {
    let mut t = String::from("needle\n");
    t.push_str(&"filler line\n".repeat(200_000));
    t.push_str("needle\n");
    let mt = m("needle", MatchMode::Text, false);
    let first = mt.find_prev(&t, t.len() - 3, false).unwrap().unwrap();
    assert_eq!(first.range.start, t.len() - 7);
    let back = mt.find_prev(&t, first.range.start, false).unwrap().unwrap();
    assert_eq!(back.range, 0..6);
}

#[test]
fn empty_matches_and_replace_all() {
    let t = "a\nb\n";
    let mt = m("^", MatchMode::Regex, true);
    let rep = mt.compile_replacement("> ").unwrap();
    let r = mt.replace_all(t, None, &rep, None).unwrap();
    assert_eq!(apply_edits(t, &r.edits).unwrap(), "> a\n> b\n> ");
    let star = m("x*", MatchMode::Regex, true);
    assert_eq!(star.find_all("axxb").unwrap(), vec![0..0, 1..3, 4..4]);
    // Unicode: empty matches advance by whole chars.
    assert_eq!(star.find_all("é𝄞").unwrap().len(), 3);
}

#[test]
fn replace_templates() {
    let t = "john smith, jane doe";
    let mt = m(r"(\w+) (?<last>\w+)", MatchMode::Regex, true);
    let rep = mt.compile_replacement(r"\u$1 \U${last}\E!").unwrap();
    let r = mt.replace_all(t, None, &rep, None).unwrap();
    assert_eq!(r.count, 2);
    assert_eq!(apply_edits(t, &r.edits).unwrap(), "John SMITH!, Jane DOE!");
    assert_eq!(
        mt.compile_replacement("${zzz}"),
        Err(MatchError::UnknownGroup("zzz".into()))
    );
    // Text mode: replacement is literal.
    let lit = m("a", MatchMode::Text, true);
    let rep = lit.compile_replacement("$1\\U").unwrap();
    assert_eq!(
        apply_edits(
            "aa",
            &lit.replace_all("aa", None, &rep, None).unwrap().edits
        )
        .unwrap(),
        "$1\\U$1\\U"
    );
}

#[test]
fn replace_in_selection_and_limit() {
    let t = "aa aa aa aa";
    let mt = m("aa", MatchMode::Text, true);
    let rep = mt.compile_replacement("b").unwrap();
    let r = mt.replace_all(t, Some(3..8), &rep, None).unwrap();
    assert_eq!(apply_edits(t, &r.edits).unwrap(), "aa b b aa");
    let r = mt.replace_all(t, None, &rep, Some(1)).unwrap();
    assert_eq!(apply_edits(t, &r.edits).unwrap(), "b aa aa aa");
}

#[test]
fn replace_selection_step() {
    let t = "x1 x2";
    let mt = m(r"x(\d)", MatchMode::Regex, true);
    let rep = mt.compile_replacement("y$1$1").unwrap();
    let step = mt.replace_selection(t, &(0..2), &rep).unwrap();
    assert_eq!(step.edit.as_ref().unwrap().text, "y11");
    let none = mt.replace_selection(t, &(1..2), &rep).unwrap();
    assert!(none.edit.is_none());
}

#[test]
fn lookbehind_sees_context_in_find_at_and_in_selection() {
    let t = "foo bar";
    let mt = m(r"(?<=foo )bar", MatchMode::Regex, true);
    assert_eq!(mt.find_at(t, 4).unwrap(), Some(4..7));
    assert_eq!(mt.find_all_in(t, 4..7).unwrap(), vec![4..7]);
    assert!(mt.find_all_in(t, 0..5).unwrap().is_empty());
}

#[test]
fn unicode_text() {
    let t = "日本語のテキスト 😀 e\u{301}clair ÉCLAIR";
    let jp = m("テキスト", MatchMode::Text, true);
    let r = jp.find_all(t).unwrap();
    assert_eq!(&t[r[0].clone()], "テキスト");
    let emoji = m("😀", MatchMode::Text, true);
    let r = emoji.find_all(t).unwrap();
    let idx = LineIndex::new(t);
    // "日本語のテキスト " = 8 UTF-16 units
    assert_eq!(idx.position(t, r[0].start), Position::new(0, 9));
    // ignore case across Unicode
    assert_eq!(all(&m("éclair", MatchMode::Text, true), t), ["ÉCLAIR"]);
    // combining mark: decomposed e + U+0301 is not found by precomposed query (no normalisation)
    assert!(
        m("\u{e9}clair", MatchMode::Text, true)
            .find_all("e\u{301}clair")
            .unwrap()
            .is_empty()
    );
    // word chars in CJK
    assert_eq!(
        m("語", MatchMode::ExactWord, true)
            .find_all("日本語 語 語の")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn crlf_mode() {
    let t = "one\r\ntwo\r\nthree";
    let base = m(r"^t\w+$", MatchMode::Regex, true);
    assert_eq!(all(&base, t), ["three"]); // `$` cannot see \r\n without crlf
    let crlf = base.with_crlf(true);
    assert_eq!(all(&crlf, t), ["two", "three"]);
    let multi = m(r"one\ntwo", MatchMode::Regex, true).with_crlf(true);
    assert_eq!(all(&multi, t), ["one\r\ntwo"]);
    let text_ml = m("one\ntwo", MatchMode::Text, true).with_crlf(true);
    assert_eq!(text_ml.find_all(t).unwrap().len(), 1);
    let dot = m(r"o.*", MatchMode::Regex, true).with_crlf(true);
    assert_eq!(all(&dot, t), ["one", "o"]);
    // replacement newlines become CRLF
    let rep = m("two", MatchMode::Text, true).with_crlf(true);
    let r = rep.compile_replacement("a\nb").unwrap();
    let edits = rep.replace_all(t, None, &r, None).unwrap().edits;
    assert_eq!(edits[0].text, "a\r\nb");
}

#[test]
fn wrap_around_option_helpers() {
    let opts = MatcherOptions {
        mode: MatchMode::Text,
        ignore_case: true,
        wrap_around: false,
        ..Default::default()
    };
    let mt = Matcher::new("ab", &opts).unwrap();
    let t = "ab ab";
    assert_eq!(mt.find_next_wrapping(t, 4).unwrap(), None);
    assert_eq!(mt.find_prev_wrapping(t, 0).unwrap(), None);
    let opts = MatcherOptions {
        wrap_around: true,
        ..opts
    };
    let mt = Matcher::new("ab", &opts).unwrap();
    assert_eq!(mt.find_next_wrapping(t, 4).unwrap().unwrap().range, 0..2);
    assert_eq!(mt.find_prev_wrapping(t, 0).unwrap().unwrap().range, 3..5);
}
