# scratch-search

ScratchRunner's search engine: the in-editor find bar, project search &
replace, fuzzy matching, QuickNav (Open Quickly) providers and symbol
extraction. NSH-free, so it builds and tests without a window server.

```
cargo test -p scratch-search
```

## Modules / public API

| Module | Main API |
|--|--|
| `matcher` | `Matcher` (one compiled search), `MatcherOptions`, `MatchMode` (Text, Exact Word, Word Starts With, Word Ends With, Ignoring Whitespace, Regular Expression), `Found`, `Matches`, `ReplaceAll`, `ReplaceStep`, `MatchError`; `find_at`, `iter`, `iter_range`, `find_all`, `find_all_in`, `find_limited`, `count`, `find_next`, `find_prev`, `is_match_at`, `replacement_for`, `replace_selection`, `replace_all`, `with_crlf` |
| `replacement` | `Replacement` templates: `$1`, `${name}`, `$0`/`$&`, `\1`, `\n`, `\t`, and `\U \L \E \u \l` |
| `text` | `Position`, `PositionRange`, `LineIndex`, `utf16_len`, `byte_to_utf16`, `utf16_to_byte`, boundary helpers |
| `edit` | `TextEdit`, `PositionEdit`, `apply_edits`, `validate_edits`, `to_position_edits`, `invert_edits`, `EditError` |
| `patterns` | Insert Pattern snippets (`builtin_find_patterns`, `builtin_replace_patterns`, `replace_insertions_for`, `current_search_as_regex`, `escaped_replacement`) and persisted `UserPatterns` (`TextFinderUserPatterns`) |
| `onig` | `translate` for Oniguruma constructs fancy-regex lacks (`\h`, `\Z`, `\R`, Ruby `(?m)`, CRLF `$`/`.`/`\n`) |
| `fuzzy` | `Fuzzy` (reusable scorer), `fuzzy_match`, `rank`, `rank_cancellable`, `FuzzyMatch` with byte positions and `ranges`, `CaseMatching` |
| `project::search` | `SearchRequest`, `SearchSource` (`Folder`, `SelectedItems`, `CustomPaths`, `OpenDocuments`), `DocumentSnapshot`, `SearchMode`, `SearchOptions` (subdirectories, hidden, binary, include/exclude extensions, excluded dirs, scope selector + optional `ScopeFilter`, max file size, fallback encoding), `search_project`, `spawn_search`, `SearchHandle` (`cancel`, `recv`, `recv_timeout`, `recv_batch`, `join`), `SearchEvent` (`FileStart`, `Match`, `Progress`, `Done`), `SearchResults`, `SearchFile`, `ProjectMatch`, stable `FileId`/`MatchId` |
| `project::replace` | `ReplacePlan` (built from results + matcher + `Replacement` + `TargetSelection`, per-match `set_accepted`, `accept_all`, `reject_all`, `summary`, `rename_conflicts`), `ReplaceOptions` (dry run, `ConflictPolicy`, binary), `ReplaceReport`, `ReplaceJournal` (`undo`), `UndoReport` |
| `project::encoding` | `TextEncoding`, `decode`, `decode_as`, `encode`, `roundtrips`, `looks_binary`, `content_hash`, `default_fallback` |
| `quicknav` | `QuickNavQuery` → `quicknav` → `Vec<QuickNavItem>`; providers `line_items`, `parse_line_spec`, `complete_paths`, `tab_complete_path`, `path_items`, `FileCache` (cached walk + `invalidate` watcher hook + `generation`), `RecentDocuments`, `recent_items`, `open_document_items`, `symbol_items`, `language_items`, `menu_items`; `QuickNavKind`, `IconKind`, `QuickNavAction`, `QuickNavMode` |
| `symbols` | `extract_symbols`, `extract_symbols_with`, `LanguageId`, `SymbolKind`, `Symbol`, `SymbolOptions`, `detect_language`, `SUPPORTED_LANGUAGES` |
| `cancel` | `CancelToken` |

## Behaviour notes

* **Coordinates.** All engines work on `&str` and return **byte offsets**.
  `LineIndex` converts to the editor's 0-based line / UTF-16 column
  positions (and back), including CJK, emoji surrogate pairs and combining
  marks (no normalisation is performed).
* **CRLF.** `Matcher::with_crlf(true)` makes `$`, `.` and `\n` understand
  `\r\n` and writes replacement newlines as CRLF. Project search detects
  CRLF per file and wraps the matcher automatically.
* **Regex flavour.** `fancy-regex` in Oniguruma mode: look-around,
  backreferences, named groups, atomic groups, `\h`, `\Z`, `\R`; Ruby
  `(?m)` means dot-matches-newline. Invalid patterns return
  `MatchError::InvalidRegex`; catastrophic backtracking returns
  `MatchError::BacktrackLimit` (budget in `MatcherOptions`).
* **Encodings.** Project search decodes BOM-marked UTF-8/16, valid UTF-8,
  then the configured fallback (windows-1252 by default). Replace writes
  with the encoding detected at search time, verifies the raw bytes have
  not changed (and reports `EncodingChanged` when the encoding/BOM changed),
  refuses lossy decodes and unmappable output, and writes through an
  exclusively-created temporary file while preserving permissions and extended
  attributes. Replacements through symbolic links are refused, and files are
  rechecked immediately before the atomic rename.
* **Cancellation and batching.** `CancelToken` is checked per file and per
  match; `SearchHandle::recv_batch(max, wait)` is the UI's per-run-loop
  batching primitive. The final `SearchEvent::Done` carries the outcome
  (`Completed`, `Cancelled`, `LimitReached`, `Failed`), counters, elapsed
  time and walk errors.
* **Replace.** `ReplacePlan::execute` expands capture groups at execution
  time, detects overlapping edits, applies accepted content matches, then
  renames files **and folders** in `Filenames` mode (conflict policy `Skip`
  / `Overwrite` / `Fail`), and records writes and renames in a
  `ReplaceJournal` ("Undo Replace") that refuses to clobber files edited
  after the replace. When overwriting a rename target, the original is moved
  aside in the same directory and restored by undo, preserving its filesystem
  metadata. If undo fails or is cancelled, `UndoReport::remaining` retains the
  remaining steps for retry. Backups use `.scratch-replace-backup-*` names and
  remain on disk if the journal is dropped or the application exits; they can
  be recovered manually, or removed when no longer needed. Filename renames
  currently require macOS or Linux non-overwriting rename support.
* **QuickNav.** Providers are pure functions over UI-supplied data.
  `FileCache` walks the navigator root lazily and is invalidated by the
  filesystem watcher (`invalidate`); `generation` lets a UI skip redraws.
  Result ids are stable (used for "remember last selected result").
* **Symbols.** Line-based heuristic regex tables per language with brace /
  indentation / heading nesting. The 29 bundled CodeRunner languages map to
  24 extractors (C# variants share one, JavaScript variants share one,
  TypeScript variants share one, Objective-C and Objective-C++ are
  distinct). Plain text has no symbols.

## Not implemented here

* The `scope_selector` string is stored but not interpreted: this crate has
  no grammar/highlighter, so scope-restricted search takes an optional
  `SearchOptions::scope_filter` predicate from the caller.
* Symbol extraction is heuristic (no full parser), so unusual formatting can
  hide or misclassify a symbol.
