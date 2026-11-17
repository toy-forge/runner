# scratch-debugger

ScratchRunner's debugger back end.  It drives ordinary command-line debuggers
(LLDB, pdb, node inspect, jdb, bashdb, perl -d, phpdbg) through a small TOML
adapter format authored for ScratchRunner.  It does **not** use CodeRunner's
proprietary `.crDebugger` bundles, helper binaries, icons or scripts, and it has
no NSH/UI dependency.

The crate is deliberately UI-free: a front end starts a [`DebugSession`],
sends commands and receives typed [`DebugEvent`]s.

## Quick start

```rust,no_run
use std::path::PathBuf;
use std::time::Duration;
use scratch_debugger::*;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let registry = AdapterRegistry::builtin();
let adapter = registry.for_language("c").expect("a C debugger").clone();
let config = SessionConfig::new(adapter, PathBuf::from("/tmp/a.out"))
    .cwd(PathBuf::from("/tmp"))
    .breakpoints(vec![Breakpoint::new("/tmp/a.c", 5)]);

let session = DebugSession::start(config)?;
loop {
    match session.recv_timeout(Duration::from_secs(5)) {
        Ok(DebugEvent::StoppedAt { location, annotation, .. }) => {
            println!("{annotation} at {}", location.describe());
            session.variables(VariablesQuery::new(location.frame))?;
        }
        Ok(DebugEvent::VariablesChanged { variables, .. }) => {
            for root in &variables.roots {
                println!("{} = {:?}", root.name, root.value);
            }
            session.continue_()?;
        }
        Ok(DebugEvent::Exited { code }) => break,
        Ok(_) => {}
        Err(_) => break,
    }
}
# Ok(())
# }
```

## Adapter format

An adapter is a TOML document embedded at compile time (for the built-ins) or
loaded from a user directory with `AdapterRegistry::load_user_dir`.  It names
the debugger, its languages, its prompt, the command templates for each verb,
and the regular expressions used to parse its output.  Templates use
`{placeholder}` substitution; regexes may use named groups such as
`(?<file>...)`, `(?<line>...)`, `(?<function>...)`.

```toml
id = "my-debugger"
name = "My Debugger"
languages = ["c"]
prompt = '(?m)\(mydbg\) $'
termination = 'exited with code (\d+)'

[start]
program = "mydbg"
args = ["--quiet", "{program}", "{program_args}"]

[parsers]
kind = "generic"
stop_location = '(?m)^at (?<file>.+):(?<line>\d+)$'
frames = '^#(?<index>\d+) (?<name>\S+) at (?<file>.+):(?<line>\d+)$'

[commands]
launch = "run"
continue = "continue"
step_over = "next"
step_in = "step"
step_out = "finish"
breakpoint_set = "break {file}:{line}"
breakpoint_clear = "clear {file}:{line}"
backtrace = "where"
variables = "info locals"
evaluate = "print {expr}"
quit = "quit"
```

The same model describes a debugger's prompts (with a PTY, without readline),
its echo behaviour, whether a prompt means "paused", setup commands, whether
program stdin/stdout share the debugger's terminal, and the regexes for stop
locations, frames, threads, variables, breakpoint hits and crashes.

See [`src/adapter.rs`](src/adapter.rs) for every field and the placeholder set.
Both `DebuggerAdapter::validate` and the built-in adapters are covered by unit
tests, so a malformed regex or missing helper is reported at load time instead
of at the first stop.

### Built-in adapters

| id | languages | status |
|--|--|--|
| `lldb` | C, C++, Objective-C, Objective-C++, Swift, Rust, Go | Full support, live-tested. |
| `pdb` | Python | Full support, live-tested. |
| `node` | JavaScript (Node) | Best effort, live-tested: breakpoints, stepping, evaluation, backtraces and program output; node's inspector has no portable variable-tree command. |
| `jdb` | Java | Best effort: attaches to a JDWP socket spawned by the session. |
| `bashdb` | Shell | Best effort; install hint when the binary is missing. |
| `perl` | Perl | Best effort: `perl -d`; lexical dumps surface as debugger output. |
| `phpdbg` | PHP | Best effort; reports a clear "not available" status when PHP 8 lacks phpdbg. |

`AdapterRegistry::for_language` picks the default for a language and
`DebuggerAdapter::availability` / `status_text` expose whether the underlying
tool is installed, so the UI can show "not available" before starting a session.

## Sessions

`DebugSession` runs one debugger in an actor thread and exposes a blocking,
thread-safe command API.  All state changes arrive on an `mpsc` channel:

- `DebugEvent::State` – `Starting`, `Running`, `Paused { location, frame, reason }`,
  `Exited { code }`, `Crashed`, `Error`, `Terminated`
- `DebugEvent::StoppedAt` – location, structured `StopReason` and the
  CodeRunner-style annotation ("Stopped at Breakpoint", "Stopped at Temporary
  Breakpoint", "Paused Execution", …)
- `DebugEvent::StackChanged`, `VariablesChanged`, `WatchValues`,
  `ExpressionResult`, `BreakpointsChanged`
- `DebugEvent::Output` (program stdout/stderr), `DebuggerOutput`, `LogLine`
  (verbose mode, both directions), `Exited`, `Error`

Commands cover set/clear breakpoints, continue, pause (adapter command or
SIGINT), step over/in/out, frame selection, variables, lazy variable expansion,
value editing, print description (`po`/`pp`), expression evaluation, watches,
REPL commands and forwarding program stdin.

Program input and debugger commands share a channel; the actor keeps them
separate and forwards input to the inferior (a second PTY for LLDB, the shared
pipe for pdb).

## Variables

A variable tree is a list of [`Variable`] roots.  Every node has a stable
`VariableId` (a path of indices) plus `name`, `type_name`, `value`,
`description`, `raw_value`, `expr`, `children` and flags for `expandable` and
`truncated`.  `DebugSession::expand_variable` fetches just that subtree and
emits a `VariablesChanged` whose `parent` is the expanded node.

`SortMode` provides the three ScratchRunner orders: `Name` (0), `Natural` (1,
numeric-aware) and `ReverseNatural` (2).  Sorting is stable and recursive.

## Breakpoints and line tracking

`Breakpoint` carries file, line, condition, enabled, temporary and ghost flags;
`BreakpointStore` handles adding, toggling, deduplicating, consuming temporary
breakpoints and persistence as TOML or JSON:

```toml
[[breakpoint]]
id = 3
file = "src/main.c"
line = 12
condition = "i == 3"
enabled = true
temporary = false
ghost = false
```

`LineTracker` maps breakpoint lines through a text edit described by
`TextEdit::replace` / `insert` / `delete`, so an editor can move breakpoints as
the user types.  `SourceMap` resolves paths reported by a debugger to local
files (absolute paths, canonical paths and unique file-name matches).

## Process hygiene

The actor owns every child process.  Debuggers are spawned in their own process
group (and with a PTY when needed), the debugger's job server is shut down, the
whole group is terminated on quit, drop or error, and temporary session files
are removed.  Only processes started by the session are ever signalled.

## Tests

```
cargo test -p scratch-debugger
```

- 51 unit tests cover adapters, parsers (with captured transcripts in
  `tests/fixtures`), variables, sorting, breakpoints, line tracking and the REPL
  history.
- `tests/lldb_session.rs` compiles small C programs with `clang -g` and drives
  the installed LLDB: breakpoints, conditions, temporary breakpoints, locals,
  lazy struct expansion, step over/in/out, evaluate/print/edit, watches, REPL,
  stdin forwarding, crashes and pause.
- `tests/pdb_session.rs` drives `python3 -m pdb` through the same scenarios,
  including a post-mortem uncaught exception and program input.
- `tests/node_session.rs` drives `node inspect` through a breakpoint,
  expression evaluation, step over and program output.

Tests that need `clang`, `lldb`, `python3` or `node` print a visible "skipping
test" message and pass when the tool is absent.  Every live test uses its own
temporary directory and its own debugger process, so the suite also passes with
cargo's default parallel test threads; no test touches a user's running
application or data.
