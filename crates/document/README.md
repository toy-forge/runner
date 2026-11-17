# scratch-document

Documents, files, sessions and persistence for ScratchRunner. NSH-free (fast `cargo test`).
Depends on `scratch-language` (language detection, templates) and `scratch-config` (paths, preferences).
The crate docs (`cargo doc -p scratch-document --open`) have the type table and a worked example.

| Module | What it does |
|---|---|
| `document` | `Document`: id, path, untitled flag, language (+lock, grammar override), encoding, BOM, line ending, read-only/locked, dirty, created/updated, revision, per-file `RunSettings`, `Breakpoints`, `ViewState` (selections, scroll, folds), `FileStamp`. Transitions: `from_file`, `mark_edited`, `mark_saved`, `relocate` (Save As), `set_language`, `redetect_language`, `to_meta`/`apply_meta`. |
| `encoding` | `TextEncoding` (24 encodings; labels = preference labels, IANA names, `NSStringEncoding`, `CFStringEncoding`, `com.apple.TextEncoding` value). `detect_encoding` (BOM > UTF-16 NUL pattern > xattr > XML declaration > Python cookie > default > valid UTF-8 > Windows-1252), strict/lossy `decode`, `encode` with `Unrepresentable` (first lost character, line, column), `unrepresentable_characters`, `reinterpret`, `convert`, `check_insert` (Insert / Promote to UTF-8). UTF-16/32 and ISO-2022-JP are implemented here (encoding_rs cannot encode them). |
| `line_ending` | `LineEnding` (`Lf`/`CrLf`/`Cr`), `detect` (counts, dominant, mixed), `normalize` to `\n`, `apply`. |
| `fileio` | `load_text` (large-file guard + `allow_large`, decoding, normalising, `FileStamp`, access), `save_text` (re-applies line endings, refuses lossy encodings, **atomic**, keeps permissions and xattrs, writes through symlinks, `Locked`/`ReadOnly` errors, writes `com.apple.TextEncoding`), `file_access` (`chflags uchg` = `Locked`), `run_location`/`copy_for_run` (run a locked file from a temp copy), `write_atomic_preserving`, `copy_xattrs`. |
| `meta` | `MetaStore` + `FileMeta`: xattrs `ScratchRunner:{Language,Grammar,RunCommand,Arguments,CompilerFlags,Input,Breakpoints,Diagnostics,Selection}`, read-compat `CodeRunner:*` (`CodeRunnerCompat::{Off,ReadOnly(default),ReadWrite}`), JSON sidecar (`<dir>/<hash>-<name>.json`) when the file refuses attributes (FAT, locked, too large). |
| `unsaved` | `UnsavedStore`: untitled documents are real files (`Untitled.py`, `Untitled 2.py`) in the Unsaved directory with a manifest under `.scratchrunner/`; `create_untitled`/`create_first_launch`/`create_from_clipboard`, `autosave`, `recover` (crash recovery incl. unsaved edits of real files with `DiskState`), `discard`, `finish_save_as`, `retarget_untitled` (language change renames the extension), `cleanup_orphans`. `AutosaveScheduler`: pure debounce with max wait. |
| `draft` | `DraftStore`/`DraftRecord`: JSON snapshots with a recovery copy of the previous version (kept from the old crate). |
| `session` | `Session` v2 (versioned, v1 migrated): `WindowState` {frame, fullscreen, `Chrome` panel visibility/sizes, `SidebarState` root+mode, `TabState`s, active, `NavigationHistory`}; tab rules (`insert_tab` by `InsertNewTabs`, close selects neighbour, next/previous wrap, move); `prune_unavailable`; `WindowDefaults` (`window-state.json`), `cascade_frame`, `clamp_to_screens`. |
| `recent` | `RecentDocuments`: capped, newest first, `clear`, existence filter, `labels()` (folder shown only for equal names), per-app JSON. |
| `naming` | `next_untitled_name`, `retarget_untitled_name`, `duplicate_name` (`a copy.py`, `a copy 2.py`), `numbered_name`, `validate_file_name`. |
| `fileops` | `rename`/`move_to`/`copy_to`/`duplicate`/`create_file`/`create_folder` with `FileOpError::Conflict` instead of silent overwrite, xattrs carried, `trash_item` (system Trash, fallback `~/.Trash`), `save_panel_defaults`. |
| `external` | `check_external(path, &stamp)` (mtime+size, then content hash) -> `ExternalState`; `decide(state, dirty)` -> `SilentReload` / `Conflict` / `UpdateStamp` / `FileDeleted`... |
| `watch` | `WatchService`: debounced `notify` watcher on the parent directories; `events()` channel of `FileChanged`; `mute(path, dur)` for your own saves. |
| `history` | `NavigationHistory`: back/forward over `(document, caret)`; CodeRunner titles (`Go Back to Previous Caret Location`, `... to Search Results`, `... to Tab or Document "x"`); pruned on close. |
| `closing` | `plan_close` -> `ClosePlan` (ordered `Prompt`s + silent actions) -> `CloseFlow` (answers) -> `CloseOutcome`; `ClosePolicy::from_preferences`; `close_command_title`. |

## Decisions worth knowing

* The editor text always uses `\n`; encoding, BOM and the original line ending live on the `Document`.
* Positions are NSH's (`line`, UTF-16 `column`, both from 0), so selections/folds/scroll convert 1:1.
* Atomic saves replace the inode, so permissions and **all extended attributes** (our metadata, Finder tags, `com.apple.TextEncoding`) are copied to the new file before the rename.
* A document with a file of its own and unsaved edits is recovered from a *draft* (full text), never by touching the user's file; an untitled document is recovered from its file in `Unsaved/`.
* `cleanup_orphans` never deletes a non-empty untitled file.
* `CodeRunner:*` attribute payloads other than plain strings are undocumented: the parsers accept JSON, plists and bare numbers, and CodeRunner's line numbers are assumed to be 1-based (converted to 0-based). `CodeRunner:Language` holds CodeRunner's language UUID (verified on a real file: `8CF92C94-...`); `com.apple.TextEncoding` is `utf-8;134217984`.
* Duplicate naming follows the Finder (`name copy.ext`, `name copy 2.ext`).
* FSEvents reports a modification when the writer closes the file; the watcher therefore sees in-place writes after the editor closes the handle. Atomic replace is reported as soon as it happens.
* Large-file limit default: 16 MiB (`DEFAULT_LARGE_FILE_LIMIT`, `ReadOptions::large_file_limit`).

## Tests

`cargo test -p scratch-document`: unit tests in every module, `tests/lifecycle.rs` (crash recovery -> Save As -> metadata -> external change -> session), doctests. Tests that need extended attributes, `chflags` or a file watcher skip visibly when the environment lacks them. No test touches the real Trash, the user's Application Support, or any CodeRunner data.
