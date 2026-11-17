# ScratchRunner

A native macOS scratchpad for writing and running code. Open it, type, press ⌘R, read the
result. No project is needed, and unsaved text is kept across restarts.

- 28 languages run in place — Python, Shell, JavaScript, TypeScript, Rust, Go, C, C++,
  Objective-C, Swift, Java, Kotlin, Ruby, Perl, PHP, Lua, Haskell, C#, AppleScript,
  LaTeX and more. The console shows stdout, stderr, the exit status and the time taken.
  ⌘. stops a run.
- Language servers (Pyright for Python, and the others when installed) add completions,
  hovers and diagnostics. They start only when a document needs one.
- A built-in debugger (lldb, pdb, node, jdb, bashdb, perl, phpdbg) runs scripts with
  breakpoints, a stack and variables pane, and console stepping.
- Unsaved text is written to drafts about 1.5 seconds after the last edit.

Specification: [SPEC.md](SPEC.md). Architecture: [docs/architecture.md](docs/architecture.md).
Building and testing: [docs/development.md](docs/development.md).

## Keys

| Key | Action |
|---|---|
| ⌘N, ⌘T | New scratchpad |
| ⌘O | Open a file |
| ⇧⌘O | Open Quickly: tabs, files under the navigator root, or a typed path |
| ⌘S, ⇧⌘S | Save, Save As |
| ⌘W | Close the tab |
| ⌘1…9 | Switch tab |
| ⌘R | Run |
| ⌘. | Stop |
| ⇧⌘R | Run Settings: command, flags, arguments, input, environment, folder |
| ⌥⌘R | Run and Debug |
| ⌃⌘R | Run Without Debugging |
| ⌘B | Compile / Run Diagnostics |
| ⌘K | Clear the console |
| ⇧⌘C | Show or hide the console |
| ⇧⌘H | Show or hide the navigator |
| ⇧⌘I | Show or hide the documentation view |
| ⌘, | Settings |
| ⌘F, ⌥⌘F | Find, Find and Replace |
| ⌘G, ⇧⌘G | Find Next, Find Previous |
| ⌘E | Use the selection for Find |
| ⌘L | Go to Line |
| ⌘/ | Un/Comment Lines |
| ⌘D | Duplicate Line |
| ⇧⌘⌫ | Delete Line |
| ⌥⌘↑ / ⌥⌘↓ | Swap Lines Up / Down |
| ⌃S | Select Line |
| ⌘+, ⌘−, ⌘0 | Bigger, Smaller, Actual Size |
| ⌃Space | Completions (with a language server) |
| ⌥Esc | Completions |

## Workbench motion inventory

The first workbench targets macOS desktop. Toolbar and tab hover/press use the
shared fast color and opacity transitions; selected tabs use a persistent fill.
Native language menus and save/close dialogs use system motion. The console
split follows the pointer directly; output, typing, caret movement, status and
streaming output update without animation. Console visibility and tab changes
are immediate in the first integration. Reduced motion suppresses transition
motion. Visual and accessibility verification is tracked separately from build
and behavior checks.

## Build

```sh
nsh dev                        # hot reload, on this Mac
nsh run                        # the app as it runs when launched
nsh check                      # cargo check, then the NSH type checks
nsh package --target macos     # dist/ ... the app bundle, built by the NSH CLI
cargo run                      # the root directly, without the NSH CLI
```

The project follows the NSH layout: `nsh.json` at the root names the crate (`"rust": "."`) and
the root component (`"root": "ScratchRunner"`), and `src/lib.rs` registers it as a native module.
