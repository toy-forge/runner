//! The compound-command language and its interpreter.
//!
//! A compound command is a JSON program (comments and trailing commas allowed) stored in a key
//! binding. [`CompoundCommand::parse`] reads it, [`Machine`] runs it against any editor that
//! implements [`Host`]. [`fake::FakeHost`] is a plain-string host for tests.
//!
//! ```
//! use std::sync::Arc;
//! use scratch_commands::compound::{CommandLibrary, CompoundCommand, Machine, Registers, Status, fake::FakeHost};
//!
//! // Typing `{` at the end of a line inserts the pair and parks the caret between.
//! let cmd = CompoundCommand::parse(r#"[
//!     "testSucceedingText:", "\n",      // only at the end of a line
//!     "insertText:", "{}",
//!     "moveLeft",
//! ]"#).unwrap();
//! let mut host = FakeHost::new("fn main() \nx");
//! host.set_caret(10);
//! let mut machine = Machine::new(Arc::new(CommandLibrary::new()));
//! let status = machine.start_command(&cmd, None, &mut host, &mut Registers::new());
//! assert_eq!(status, Status::Consumed);
//! assert_eq!(host.text_string(), "fn main() {}\nx");
//! assert_eq!(host.caret(), 11);
//! ```

mod ast;
mod builtins;
pub mod fake;
mod host;
mod lenient;
mod machine;
mod registers;
mod search;

pub use ast::{Arg, Call, CommandList, CompoundCommand, Item, KeyEntry, KeyTable, Root};
pub use host::{CommandOutcome, Edge, Host, HostMenuItem, KeyInput, Selection};
pub use lenient::to_strict_json;
pub use machine::{CommandLibrary, Machine, Status};
pub use registers::{Registers, SavedSelection};

use thiserror::Error;

/// Why a compound command is invalid or failed to run.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CompoundError {
    /// The source is not valid (lenient) JSON.
    #[error("invalid JSON at line {line}, column {column}: {message}")]
    Json {
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
        /// The parser's message.
        message: String,
    },
    /// The root is neither a list nor an object.
    #[error("a compound command must be a JSON list or object")]
    BadRoot,
    /// An element has the wrong JSON type.
    #[error("{path}: expected {expected}, found {found}")]
    Structure {
        /// Location (`$[3]`).
        path: String,
        /// What was expected.
        expected: &'static str,
        /// What was found.
        found: &'static str,
    },
    /// A command with colons is missing arguments.
    #[error("command {command:?} needs {expected} argument(s); argument {index} is missing")]
    MissingArgument {
        /// The command.
        command: String,
        /// The missing argument (1-based).
        index: usize,
        /// How many arguments the command takes.
        expected: usize,
    },
    /// An argument has the wrong type.
    #[error("command {command:?}: argument {index} must be {expected}")]
    BadArgument {
        /// The command.
        command: String,
        /// The argument (1-based).
        index: usize,
        /// What it must be.
        expected: &'static str,
    },
    /// A command neither the interpreter nor the host knows.
    #[error("unknown command {0:?}")]
    UnknownCommand(String),
    /// `callCompoundCommand:` / `includeCompoundCommand:` names no compound command.
    #[error("no compound command named {0:?}")]
    UnknownCompound(String),
    /// A regular expression does not compile or failed to run.
    #[error("invalid regular expression: {0}")]
    Regex(String),
    /// The step or recursion limit was exceeded (runaway recursion).
    #[error("the compound command ran too long or recursed too deeply")]
    LimitExceeded,
}
