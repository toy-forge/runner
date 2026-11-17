//! Highlighting benchmark: `cargo run --release -p scratch-theme --example bench -- [source-dir] [target-MB]`.
//!
//! Concatenates the `.rs` files found under `source-dir` (default: this
//! crate's `src`) until the text is about `target-MB` megabytes and times the
//! engine's operations on it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use scratch_theme::{DEFAULT_DARK, Highlighter, ThemeLibrary};

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

fn rss_mb() -> f64 {
    let pid = std::process::id().to_string();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/src").to_owned());
    let target_mb: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let mut files = Vec::new();
    collect(std::path::Path::new(&dir), &mut files);
    let mut base = String::new();
    for f in &files {
        if let Ok(t) = std::fs::read_to_string(f) {
            base.push_str(&t);
            if !base.ends_with('\n') {
                base.push('\n');
            }
        }
    }
    assert!(!base.is_empty(), "no .rs files under {dir}");
    let mut text = String::new();
    while (text.len() as f64) < target_mb * 1_048_576.0 {
        text.push_str(&base);
    }
    let lines = text.matches('\n').count();
    println!("text: {:.2} MB, {lines} lines ({} source files, base {} KB)", text.len() as f64 / 1_048_576.0, files.len(), base.len() / 1024);

    let library = ThemeLibrary::new(None);
    let theme = Arc::new(library.get_or_default(DEFAULT_DARK, true).compile());
    let rss0 = rss_mb();
    let t = Instant::now();
    let mut h = Highlighter::new("Rust", theme.clone());
    println!("syntax set + highlighter: {:.1} ms", ms(t.elapsed()));

    let t = Instant::now();
    h.update_text(text.as_str());
    println!("update_text (initial, includes copy + line index): {:.1} ms", ms(t.elapsed()));

    // First screen: how long until the top of the file is coloured?
    let t = Instant::now();
    let r = h.highlight_lines(0, 80, true, Duration::from_millis(50));
    let n: usize = r.lines.iter().map(|l| l.spans.len()).sum();
    println!("first 80 lines: {:.2} ms ({n} spans, exact={})", ms(t.elapsed()), r.exact);

    // A jump to the middle before the index exists: guess.
    let t = Instant::now();
    let r = h.highlight_lines(lines / 2, lines / 2 + 80, true, Duration::from_millis(2));
    println!("jump to middle (2 ms budget): {:.2} ms, exact={}", ms(t.elapsed()), r.exact);

    // Index the whole file in 8 ms slices, as a background worker would.
    let t = Instant::now();
    let mut slices = 0;
    let mut worst = Duration::ZERO;
    loop {
        let s = Instant::now();
        let done = h.advance(Duration::from_millis(8));
        if slices > 0 {
            worst = worst.max(s.elapsed());
        }
        slices += 1;
        if done {
            break;
        }
    }
    println!("full index: {:.0} ms in {slices} slices (worst slice after the first {:.1} ms)", ms(t.elapsed()), ms(worst));
    println!("RSS: before {:.0} MB, after index {:.0} MB (delta {:.0} MB, text is {:.0} MB)", rss0, rss_mb(), rss_mb() - rss0, text.len() as f64 / 1_048_576.0);

    // Random visible ranges with a full index.
    let mut total = Duration::ZERO;
    let mut worst = Duration::ZERO;
    let probes = 200;
    let mut seed = 12345u64;
    for _ in 0..probes {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let first = (seed >> 33) as usize % (lines - 400);
        let t = Instant::now();
        let r = h.highlight_lines(first, first + 400, true, Duration::from_millis(50));
        let d = t.elapsed();
        assert!(r.exact);
        total += d;
        worst = worst.max(d);
    }
    println!("400-line range, random position, indexed: avg {:.2} ms, worst {:.2} ms", ms(total) / probes as f64, ms(worst));

    // Typing one character in the middle.
    let mid = text.len() / 2;
    let mut at = mid;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    let mut edited = text.clone();
    edited.insert(at, 'x');
    let t = Instant::now();
    let change = h.update_text(edited.as_str());
    let upd = t.elapsed();
    let first = change.map_or(0, |c| c.first_line);
    let t = Instant::now();
    let r = h.highlight_lines(first.saturating_sub(10), first + 60, true, Duration::from_millis(50));
    println!("type 1 char mid-file: update_text {:.1} ms (copy+diff+index), re-highlight 70 lines {:.2} ms, exact={}", ms(upd), ms(t.elapsed()), r.exact);
    let t = Instant::now();
    let done = h.advance(Duration::from_secs(60));
    println!("  ...then finishing the index (convergence): {:.2} ms, done={done}", ms(t.elapsed()));

    // Opening a block comment near the top: the rest of the file changes meaning.
    let mut opened = edited.clone();
    opened.insert_str(0, "/*\n");
    let t = Instant::now();
    h.update_text(opened.as_str());
    let upd = t.elapsed();
    let t = Instant::now();
    let r = h.highlight_lines(0, 80, true, Duration::from_millis(50));
    let first_screen = t.elapsed();
    let t = Instant::now();
    let mut slices = 0;
    while !h.advance(Duration::from_millis(8)) {
        slices += 1;
    }
    println!("open /* at line 0: update_text {:.1} ms, first screen {:.2} ms (exact={}), re-index {:.0} ms in {} slices", ms(upd), ms(first_screen), r.exact, ms(t.elapsed()), slices + 1);

    // Closing it again: converges after the first checkpoint past the close.
    h.update_text(edited.as_str());
    let t = Instant::now();
    let _ = h.highlight_lines(0, 80, true, Duration::from_millis(50));
    let first_screen = t.elapsed();
    let t = Instant::now();
    let mut slices = 0;
    while !h.advance(Duration::from_millis(8)) {
        slices += 1;
    }
    println!("close it again: first screen {:.2} ms, re-index {:.0} ms in {} slices", ms(first_screen), ms(t.elapsed()), slices + 1);

    // Theme switch cost.
    let other = Arc::new(library.get_or_default("Solarized Light", false).compile());
    let t = Instant::now();
    h.set_theme(other);
    let r = h.highlight_lines(1000, 1080, true, Duration::from_millis(50));
    println!("theme switch + 80 lines: {:.2} ms (exact={})", ms(t.elapsed()), r.exact);
}
