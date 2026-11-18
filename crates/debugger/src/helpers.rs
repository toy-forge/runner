//! Built-in helper files written into a session directory.
//!
//! Two helpers exist today:
//!
//! * `lldb` - a startup script that makes LLDB's output machine friendly
//!   (stable prompt, one frame per line, no source context).
//! * `pdbvars` - a Python module that runs inside `pdb` and prints a frame's
//!   variables as JSON, including lazily expandable children.
//!
//! Both are our own code; they are not copies of CodeRunner's helper binaries.

/// The LLDB startup script.  `MESSAGE` markers separate it from the embedded
/// string list below.
pub(crate) const LLDB_INIT: &str = r#"settings set auto-confirm true
settings set show-progress false
settings set show-statusline false
settings set stop-line-count-before 0
settings set stop-line-count-after 0
settings set term-width 1024
settings set use-color false
settings set prompt "(lldb) "
settings set frame-format "frame #${frame.index}: ${frame.pc} ${module.file.basename}\`${function.name} at ${line.file.fullpath}:${line.number}\n"
"#;

/// The Python variables helper executed inside `pdb`.
pub(crate) const PDB_VARS: &str = include_str!("pdbvars.py");

/// The file name of a Python helper module.
pub(crate) fn python_helper(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "pdbvars" => Some(("scratchdbg_pdbvars.py", PDB_VARS)),
        _ => None,
    }
}

pub(crate) const GDB_INIT: &str = r#"set confirm off
set startup-with-shell off
set height 0
set width 0
set filename-display absolute
define hook-stop
  frame
end
define cr_vars
  info args
  echo -\n
  info locals
end
"#;

/// The contents of a named startup file.
pub(crate) fn init_file(name: &str) -> Option<&'static str> {
    match name {
        "lldb" => Some(LLDB_INIT),
        "gdb" => Some(GDB_INIT),
        _ => None,
    }
}
