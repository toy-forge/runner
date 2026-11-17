//! Fuzz-ish property tests: chunk splitting must not change the result, random
//! bytes must never panic, and the store's invariants must hold.

use scratch_console_model::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const TOKENS: &[&[u8]] = &[
    b"hello",
    b" world ",
    b"x",
    b"\n",
    b"\r",
    b"\r\n",
    b"\t",
    b"\x08",
    b"\x1b[0m",
    b"\x1b[1;31m",
    b"\x1b[38;5;208m",
    b"\x1b[38;2;10;20;30m",
    b"\x1b[48:2::1:2:3m",
    b"\x1b[7m",
    b"\x1b[K",
    b"\x1b[2K",
    b"\x1b[5G",
    b"\x1b[3C",
    b"\x1b[2D",
    b"\x1b[?25l",
    b"\x1b[?2004h",
    b"\x1b]0;title\x07",
    b"\x1b]8;;http://x\x1b\\",
    b"\x1b(B",
    b"\x1bc",
    b"\x1b",
    b"\x1b[",
    b"\x1b[3",
    b"\x1b]",
    b"\x18",
    "\u{4e2d}\u{6587}".as_bytes(),
    "\u{1f600}".as_bytes(),
    "caf\u{e9}".as_bytes(),
    b"\xff",
    b"\xc3",
    b"\xe2\x82",
    b"\x80",
    b"main.c:12:5: error: boom",
    b"https://example.com/a.",
    b"\x00",
    b"\x7f",
];

fn generate(rng: &mut Rng) -> Vec<u8> {
    let n = 1 + rng.below(60);
    let mut out = Vec::new();
    for _ in 0..n {
        out.extend_from_slice(TOKENS[rng.below(TOKENS.len())]);
    }
    out
}

type Dump = Vec<(u64, String, Vec<(u32, Style, Stream)>, Level, bool)>;

fn dump(c: &Console) -> Dump {
    (0..c.line_count())
        .map(|i| {
            let l = c.line(i).unwrap();
            (
                l.id().0,
                l.text().to_owned(),
                l.runs().iter().filter(|r| r.len > 0).map(|r| (r.len, r.style, r.stream)).collect(),
                l.level(),
                l.is_continued(),
            )
        })
        .collect()
}

fn feed(cfg: &ConsoleConfig, chunks: &[&[u8]]) -> Dump {
    let mut c = Console::new(cfg.clone());
    for ch in chunks {
        c.append(Stream::Stdout, ch);
    }
    c.finish_streams();
    dump(&c)
}

#[test]
fn splitting_never_changes_the_result() {
    let cfg = ConsoleConfig {
        max_line_chars: 40,
        ..Default::default()
    };
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for case in 0..3000 {
        let data = generate(&mut rng);
        let whole = feed(&cfg, &[&data]);

        // Random split points.
        let mut cuts: Vec<usize> = (0..rng.below(8)).map(|_| rng.below(data.len() + 1)).collect();
        cuts.sort_unstable();
        let mut chunks: Vec<&[u8]> = Vec::new();
        let mut at = 0;
        for c in cuts {
            chunks.push(&data[at..c]);
            at = c;
        }
        chunks.push(&data[at..]);
        assert_eq!(feed(&cfg, &chunks), whole, "case {case}: {data:?} split {chunks:?}");

        // Byte by byte.
        let singles: Vec<&[u8]> = data.chunks(1).collect();
        assert_eq!(feed(&cfg, &singles), whole, "case {case}: byte-wise {data:?}");
    }
}

#[test]
fn every_two_way_split_of_tricky_inputs() {
    let cfg = ConsoleConfig::default();
    let inputs: [&[u8]; 6] = [
        "a\u{1f600}\u{4e2d}\x1b[1;31mred\x1b[0m\r\nb\x1b]0;t\x07c\n".as_bytes(),
        b"\xe2\x82\xff\xc3\x28\n",
        b"p\r10%\r50%\x1b[K\r100%\n",
        b"\x1b[38;2;1;2;3mX\x1b[48:5:9mY\n",
        b"\x1b]8;;http://a\x1b\\link\x1b]8;;\x1b\\\n",
        b"\xf0\x9f\x98\x80\xf0\x9f\x98\n",
    ];
    for data in inputs {
        let whole = feed(&cfg, &[data]);
        for cut in 0..=data.len() {
            assert_eq!(feed(&cfg, &[&data[..cut], &data[cut..]]), whole, "{data:?} cut {cut}");
        }
    }
}

#[test]
fn random_bytes_never_panic_and_keep_invariants() {
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    for _ in 0..300 {
        let mut c = Console::new(ConsoleConfig {
            max_lines: 20,
            max_bytes: 500,
            max_line_chars: 32,
            ..Default::default()
        });
        c.set_filter(Filter {
            outputs: vec![OutputFilter::ProgramStderr, OutputFilter::Errors],
            text: Some(SearchQuery::plain("e")),
        })
        .unwrap();
        for _ in 0..30 {
            let n = rng.below(200);
            let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
            let stream = Stream::ALL[rng.below(5)];
            c.append(stream, &bytes);
            if rng.below(7) == 0 {
                let id = c.first_id();
                c.set_level(id, Level::Error);
            }
            if rng.below(11) == 0 {
                c.expect_echo("abc\n");
            }
        }
        c.finish_streams();
        check_invariants(&c);
    }
}

fn check_invariants(c: &Console) {
    // Ids contiguous.
    let ids: Vec<u64> = (0..c.line_count()).map(|i| c.line(i).unwrap().id().0).collect();
    for w in ids.windows(2) {
        assert_eq!(w[0] + 1, w[1]);
    }
    if let Some(first) = ids.first() {
        assert_eq!(*first, c.first_id().0);
    }
    // Byte accounting and limits.
    let bytes: usize = (0..c.line_count())
        .filter_map(|i| c.line(i))
        .filter(|l| !l.is_open())
        .map(|l| l.len())
        .sum();
    assert_eq!(bytes, c.byte_count());
    assert!(c.line_count() <= c.config().max_lines + 1);
    // Visible list equals brute-force filtering.
    let filter = c.filter().clone();
    let compiled = filter.compile_for_test();
    let expect: Vec<u64> = (0..c.line_count())
        .filter_map(|i| c.line(i))
        .filter(|l| compiled(l))
        .map(|l| l.id().0)
        .collect();
    let got: Vec<u64> = (0..c.visible_count()).map(|i| c.visible_id(i).unwrap().0).collect();
    assert_eq!(got, expect);
    for (i, id) in got.iter().enumerate() {
        assert_eq!(c.visible_index_of(LineId(*id)), Some(i));
        assert_eq!(c.visible_line(i).unwrap().id().0, *id);
    }
    // Every line is valid UTF-8 with runs tiling it.
    for i in 0..c.line_count() {
        let l = c.line(i).unwrap();
        let total: usize = l.runs().iter().map(|r| r.len as usize).sum();
        assert_eq!(total, l.len());
        let spans = l.spans(&SpanOptions::default());
        assert_eq!(spans.iter().map(|s| s.text.len()).sum::<usize>(), l.len());
    }
}

trait CompileForTest {
    fn compile_for_test(&self) -> Box<dyn Fn(&LineView<'_>) -> bool>;
}

impl CompileForTest for Filter {
    fn compile_for_test(&self) -> Box<dyn Fn(&LineView<'_>) -> bool> {
        // Re-derive the expected predicate independently from scope names.
        let selectors: Vec<ScopeSelector> = self
            .outputs
            .iter()
            .filter_map(|o| o.selector())
            .map(|s| ScopeSelector::parse(s).unwrap())
            .collect();
        let all = self.outputs.is_empty() || self.outputs.contains(&OutputFilter::AllOutput);
        let matcher = self.text.as_ref().filter(|t| !t.text.is_empty()).map(|t| Matcher::new(t).unwrap());
        Box::new(move |l: &LineView<'_>| {
            let scopes = l.mask().scopes();
            (all || selectors.iter().any(|s| s.matches(&scopes)))
                && matcher.as_ref().is_none_or(|m| m.is_match(l.text()))
        })
    }
}

#[test]
fn spans_always_tile_the_line_with_overlays() {
    let mut rng = Rng(42);
    let mut c = Console::default();
    c.append_str(Stream::Stdout, "\x1b[31mab\u{4e2d}\x1b[0mcd https://x.y/z ef main.c:1:2 gh\n");
    let l = c.line(0).unwrap();
    let links = l.links();
    for _ in 0..500 {
        let n = l.len() + 3;
        let a = rng.below(n);
        let b = rng.below(n);
        let (s, e) = (a.min(b), a.max(b));
        let search = s..e;
        let sel = e..(e + rng.below(5));
        let spans = l.spans(&SpanOptions {
            links: &links,
            search: std::slice::from_ref(&search),
            current_search: Some(&search),
            selection: Some(&sel),
            diagnostics: &[],
        });
        assert_eq!(spans.iter().map(|s| s.text).collect::<String>(), l.text());
        for w in spans.windows(2) {
            assert_eq!(w[0].range.end, w[1].range.start);
        }
    }
}
