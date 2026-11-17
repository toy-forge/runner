# Development

## Requirements

- macOS 14 or later, with Xcode command-line tools.
- Rust 1.97 or later (`rustup`).
- Optional, for the features that use them: Python 3, Node.js, Bun, Go, `pyright-langserver`
  or `basedpyright-langserver`, `rust-analyzer`, `gopls`, `typescript-language-server`,
  `bash-language-server`.

## NSH pin

NSH is pinned to the crates.io release the installed `nsh` CLI builds against: `nsh-core`,
`nsh-base` and `nsh-platform-macos` **0.5.1** (the workspace `Cargo.toml`; `nsh dev`
generates the app crates under `.nsh/rust/` with the same `=0.5.1` requirement, so
`cargo build` and `nsh dev` compile one API). NSH is young and its API changes; check any
new NSH call against 0.5.1 (`~/.cargo/registry/src/*/nsh-core-0.5.1`, `nsh-base-0.5.1`,
`nsh-platform-macos-0.5.1`, or `cargo doc -p nsh-core`) before using it — APIs seen in a
git checkout of nsh may not exist in 0.5.1. To move the pin, change the three `nsh-*`
versions, rebuild, run the tests, and make sure the installed CLI (`nsh --version`) emits
the same version.

## Commands

```sh
cargo test                              # all unit and integration tests
cargo test -p scratch-runner            # runs real python3, bash and rustc
cargo test -p scratch-lsp               # runs pyright-langserver if it is on PATH
nsh dev                                 # the app with hot reload (the NSH layout)
nsh check                               # cargo check, then the NSH type checks
cargo run                               # the root directly, without the NSH CLI
nsh package --target macos              # the app bundle, via the NSH CLI
```

## Tests

| Crate | What is tested |
|---|---|
| `scratch-config` | Defaults, the example file, invalid files fall back with a warning |
| `scratch-document` | Language detection, atomic writes, drafts and recovery, session round trip |
| `scratch-runner` | Plans per language, UTF-8 decoding, output limits, stdin, timeout and cancel killing the process group, compile errors, a missing interpreter |
| `scratch-lsp` | Framing, protocol parsers, and a live Pyright session (diagnostics, completion, hover, clearing diagnostics after a change) |

The app crate has no automated UI tests yet. The window has to be checked by running the app
and using it.

## Packaging

The app is packaged with the NSH CLI. `nsh.json` at the repository root names the crate (`"rust": "."`)
and its root component (`"root": "ScratchRunner"`):

```sh
nsh package --target macos
```

The output is under `.nsh/out/` (or `--out`). NSH ad-hoc signs it. A build for other Macs needs a
Developer ID identity through the CLI's `--sign` option. The `.nsh/` folder is generated and ignored by git.

## Settings

`~/Library/Application Support/ScratchRunner/settings.toml`. Every key has a default. A file
that does not parse is ignored with a warning in the status bar. The keys are the ones in
the specification's section 11, plus `[lsp.<language>]` tables with `enabled`, `command` and
`args`.

Templates: `[templates]` holds one string per language, keyed by its name (`python = "..."`).
It replaces the built-in template of that language, which starts a new scratchpad of that language
and a scratchpad that is still empty or still holds the previous language's template when its
language is changed. `<##>` marks where the caret goes (the first one); an empty string leaves the
document empty. Built-in templates are in `crates/document/src/language.rs` (`LanguageId::template`).

## Adding a language

1. Add a variant to `LanguageId` (`crates/document/src/language.rs`) with its names,
   extension and whether it runs.
2. Add its run plan in `crates/runner/src/languages.rs`.
3. If it has a language server, add a `Definition` in `crates/lsp/src/manager.rs` and a
   mapping in `server_for` (`src/lsp.rs`).
