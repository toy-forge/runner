# scratch-console-model

UI-free console model for ScratchRunner: ANSI/VT parsing, a bounded
stream-tagged line store, filters, links, selection and search. No NSH
dependency, so `cargo test -p scratch-console-model` is fast.

The crate-level docs in `src/lib.rs` have a runnable example.

## The 60-second version

```rust
use scratch_console_model::*;

let mut console = Console::new(ConsoleConfig::default());

// Feed bytes exactly as they come from a pipe: any chunking is fine.
console.append(Stream::Stdout, b"building\x1b[32m ok\x1b[0m\r\n");
console.append(Stream::Stderr, b"main.c:3:5: error: boom\n");
console.expect_echo("hello\r");              // tag the PTY's echo as Input
console.append(Stream::Stdout, b"hello\r\n"); // the echo, now Stream::Input

// Render in a virtual_list:
for i in 0..console.visible_count() {
    let line = console.visible_line(i).unwrap(); // Option; never panics in the model
    let _text = line.text();                     // tabs expanded, no escapes
    let _spans = line.spans(&SpanOptions {
        links: &line.links(),
        ..Default::default()
    });
    let _level = line.level();                   // Error / Warning / Notice
}

// The output menu + filter field:
console.set_filter(Filter::output(OutputFilter::ProgramStderr)).unwrap();

// Find, selection, copy:
let m = Matcher::new(&SearchQuery::plain("boom")).unwrap();
let _hit = console.find_next(&m, None, true);
let sel = console.select_all().unwrap();
let _copied = console.selection_text(&sel);      // respects the active filter
```

## Rules of the model

* Streams: `Stdout`, `Stderr`, `Input`, `Debugger`, `System`. A line holds
  runs; each run carries its own stream, so a prompt and the typed echo on
  one line keep their colours.
* `Console::append(stream, bytes)` is incremental and chunk-safe (including
  split UTF-8 and split escape sequences). The last line is *open* until a
  newline; progress bars are `\r` overwrites of that open line.
* Line ids (`LineId`) are stable; indices shift when lines are evicted.
  `Console::visible_*` are the filtered views.
* Limits: 200 000 lines / 32 MiB by default; `Console::truncated()` gives
  the "… N earlier lines not displayed …" marker.
* `Change { lines_added, lines_evicted, changed, guard }` is returned by
  every append; `guard` reports the large-output warning (8 MiB) and stop
  (256 MiB) thresholds.

## Modules

| Module | Contents |
| --- | --- |
| `ansi` | `AnsiParser`, `AnsiEvent`, `strip_ansi` |
| `console` | `Console`, `ConsoleConfig`, `Change`, `GuardEvent`, `Truncated` |
| `line` | `LineId`, `LineView`, `Run`, `Span`, `SpanOptions`, `Marks` |
| `tag` | `Stream`, `Level`, `ScopeMask` |
| `filter`, `scope` | `Filter`, `OutputFilter`, `ScopeSelector` |
| `links` | `detect_links`, `Link`, `LinkTarget`, `classify_level` |
| `selection` | `ConsoleSelection`, `Pos`, `LineSelection` |
| `search`, `query` | `SearchQuery`, `Matcher`, `Match`, `Console::find_*` |
| `style` | `Style`, `Color`, `Rgb`, `Attrs`, `AnsiPalette` |
| `textutil` | byte / UTF-16 / display-column conversions |

## Tests

`cargo test -p scratch-console-model` runs 83 unit tests, 4 chunk-splitting
fuzz tests and 5 throughput/memory tests (1M lines, ANSI-heavy output, CR
progress bars, filtering with a full store). For real numbers:

```
cargo test -p scratch-console-model --release --test perf -- --nocapture
```
