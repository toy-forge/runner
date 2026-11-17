//! Throughput and memory-shape tests. Thresholds are generous so they pass in
//! debug builds; run with `--release -- --nocapture` for the real numbers.

use std::time::Instant;

use scratch_console_model::*;

fn chunked(total_lines: usize, per_chunk: usize, mut f: impl FnMut(&[u8])) -> usize {
    let mut bytes = 0;
    let mut i = 0;
    let mut buf = Vec::with_capacity(per_chunk * 24);
    while i < total_lines {
        buf.clear();
        for _ in 0..per_chunk.min(total_lines - i) {
            buf.extend_from_slice(format!("line {i} some output text\n").as_bytes());
            i += 1;
        }
        bytes += buf.len();
        f(&buf);
    }
    bytes
}

#[test]
fn one_million_lines_default_limits() {
    let mut c = Console::default();
    let t = Instant::now();
    let bytes = chunked(1_000_000, 2000, |b| {
        c.append(Stream::Stdout, b);
    });
    let dt = t.elapsed();
    eprintln!(
        "1M lines ({} MB) -> {:?} ({:.1} MB/s), retained {} lines, truncated {:?}",
        bytes >> 20,
        dt,
        bytes as f64 / 1e6 / dt.as_secs_f64(),
        c.line_count(),
        c.truncated()
    );
    assert_eq!(c.line_count(), 200_000);
    assert_eq!(c.truncated().unwrap().lines, 800_000);
    assert_eq!(c.line(c.line_count() - 1).unwrap().text(), "line 999999 some output text");
    assert!(dt.as_secs() < 60, "too slow: {dt:?}");
}

#[test]
fn one_million_lines_all_retained_with_filter_search_and_selection() {
    let mut c = Console::new(ConsoleConfig {
        max_lines: 1_000_000,
        max_bytes: 1 << 30,
        warn_after_bytes: None,
        stop_after_bytes: None,
        ..Default::default()
    });
    let t = Instant::now();
    // Every 1000th line goes to stderr.
    let mut n = 0usize;
    chunked(1_000_000, 1000, |b| {
        let stream = if n.is_multiple_of(1000) { Stream::Stderr } else { Stream::Stdout };
        n += 1;
        c.append(stream, b);
    });
    eprintln!("1M lines retained: {:?}", t.elapsed());
    assert_eq!(c.line_count(), 1_000_000);

    // Random access is O(1).
    let t = Instant::now();
    let mut sum = 0usize;
    for i in (0..1_000_000).step_by(7) {
        sum += c.line(i).unwrap().len();
    }
    eprintln!("random access 143k lines: {:?} ({sum})", t.elapsed());

    // Filtering the whole store by stream (mask truth table, no text scan).
    let t = Instant::now();
    c.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();
    eprintln!("stream filter over 1M lines: {:?} -> {} visible", t.elapsed(), c.visible_count());
    assert!(c.visible_count() >= 1000);
    c.set_filter(Filter::all()).unwrap();

    // Filtering with a regex scans all text.
    let t = Instant::now();
    c.set_filter(Filter::all().with_text(SearchQuery::regex(r"line 9\d{5} "))).unwrap();
    eprintln!("regex filter over 1M lines: {:?} -> {} visible", t.elapsed(), c.visible_count());
    assert_eq!(c.visible_count(), 100_000);
    c.set_filter(Filter::all()).unwrap();

    // Find.
    let m = Matcher::new(&SearchQuery::plain("line 999999")).unwrap();
    let t = Instant::now();
    let hit = c.find_next(&m, None, false).unwrap();
    eprintln!("find last line from the top: {:?}", t.elapsed());
    assert_eq!(hit.line, LineId(999_999));

    // Copy everything.
    let t = Instant::now();
    let all = c.visible_text();
    eprintln!("copy all: {:?} ({} MB)", t.elapsed(), all.len() >> 20);
    assert!(all.starts_with("line 0 "));
    assert_eq!(all.lines().count(), 1_000_000);
}

#[test]
fn flood_with_filter_active_stays_incremental() {
    let mut c = Console::default();
    c.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();
    let t = Instant::now();
    let mut n = 0usize;
    chunked(300_000, 500, |b| {
        n += 1;
        c.append(if n.is_multiple_of(10) { Stream::Stderr } else { Stream::Stdout }, b);
    });
    eprintln!("300k lines with active filter: {:?}, visible {}", t.elapsed(), c.visible_count());
    assert_eq!(c.line_count(), 200_000);
    assert!(c.visible_count() > 0);
}

#[test]
fn ansi_heavy_output() {
    let mut c = Console::default();
    let t = Instant::now();
    let mut buf = Vec::new();
    for i in 0..200_000 {
        buf.extend_from_slice(format!("\x1b[1;3{}m[{i}]\x1b[0m \x1b[38;5;{}mcolour\x1b[0m done\n", i % 8, i % 256).as_bytes());
        if buf.len() > 60_000 {
            c.append(Stream::Stdout, &buf);
            buf.clear();
        }
    }
    c.append(Stream::Stdout, &buf);
    eprintln!("200k ANSI lines: {:?}", t.elapsed());
    assert!(c.line(c.line_count() - 1).unwrap().runs().len() >= 4);
}

#[test]
fn progress_bar_updates_stay_on_one_line() {
    let mut c = Console::default();
    let t = Instant::now();
    for i in 0..=100_000u32 {
        c.append(Stream::Stdout, format!("\r[{:>3}%] {}", i % 101, "#".repeat((i % 101) as usize / 5)).as_bytes());
    }
    c.append(Stream::Stdout, b"\n");
    eprintln!("100k CR updates: {:?}", t.elapsed());
    assert_eq!(c.line_count(), 1);
}
