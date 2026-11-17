# scratch-theme

ScratchRunner's colour themes and syntax-highlighting engine. No UI
dependency (no `nsh-*`): `cargo test -p scratch-theme` runs in seconds.

* `Theme` - TextMate `.tmTheme` model (+ CodeRunner extras: console/ANSI
  items, `gutterSettings`, `findHighlight`, ...). Parse (`from_plist`), write
  (`to_plist_xml`), import legacy `.crTheme` (`from_cr_theme`), `inverted`,
  `duplicate`, `is_dark`, `editor_colors()`, `chrome()`.
* `ThemeLibrary` - 11 built-in themes + user directory; new / duplicate /
  rename / invert / delete (= reset for built-ins) / import / export.
* `ScopeSelector` - TextMate selectors with CodeRunner's extensions
  (`( ) , | & - > * ** $ =`), scored for specificity.
* `ResolvedTheme` - compiled theme, `style_for(scope stack)`, `explain()`.
* `Highlighter` - incremental syntect highlighter: `update_text`,
  `highlight_lines`, `advance`, `scopes_at`; `Send`.

```rust
use std::{sync::Arc, time::Duration};
use scratch_theme::{Highlighter, ThemeLibrary, DEFAULT_DARK};

let library = ThemeLibrary::new(None);                    // or Some(user_dir)
let theme = Arc::new(library.get_or_default(DEFAULT_DARK, true).compile());
let mut h = Highlighter::new("Rust", theme);              // editor language name / extension
h.update_text("fn main() {}\n");                          // call again on every edit (diffed)
let r = h.highlight_lines(0, 50, true, Duration::from_millis(40));
for line in &r.lines { for span in &line.spans { /* line.line, span.col_start..col_end (UTF-16), span.style */ } }
```

Benchmark:
`cargo run --release -p scratch-theme --example bench -- <dir with .rs files> 10`.
