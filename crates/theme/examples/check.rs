use std::{sync::Arc, time::Duration};
use scratch_theme::{Highlighter, ThemeLibrary, DEFAULT_DARK};
fn main() {
    let lib = ThemeLibrary::new(None);
    let theme = lib.get_or_default(DEFAULT_DARK, true);
    eprintln!("theme: {}", theme.name);
    let resolved = Arc::new(theme.compile());
    let mut h = Highlighter::new("Shell Script", resolved);
    h.update_text("# comment\necho \"hi $USER\"\nfor i in 1 2 3; do true; done\n");
    h.advance(Duration::from_millis(100));
    let r = h.highlight_lines(0, h.line_count(), true, Duration::from_millis(50));
    eprintln!("exact={} lines={}", r.exact, r.lines.len());
    for line in &r.lines {
        eprintln!("line {} spans={:?}", line.line, line.spans.iter().map(|s| (s.col_start, s.col_end, s.style.foreground.to_hex())).collect::<Vec<_>>());
    }
}
