//! Expanding run-command templates.
//!
//! CodeRunner's run command is "a shell command with some environment
//! variables set". This module gives the two ways to execute such a template:
//!
//! * [`expand_command`]: no shell at all. The template is tokenised like a
//!   shell would (whitespace, `'...'`, `"..."`, `\x`), variables are replaced
//!   **after** tokenising, so a variable's value (a file name, a compile flag,
//!   document text) never becomes more than the characters of one argument: no
//!   word splitting, no globbing, no command injection. A list variable such
//!   as `$flags` that stands alone in a word splices into several arguments.
//!   Templates that need real shell syntax (`;`, `|`, redirects, `$(...)`,
//!   globs, `NAME=value cmd`) are refused with [`ExpandError::NeedsShell`].
//! * [`expand_shell_command`]: for templates that do need a shell (user-defined
//!   languages, `echo "<style>"; cat $filename; echo "</style>"`). Following
//!   CodeRunner, bare `$name` tokens of the run-command variables are replaced
//!   by their single-quoted, escaped value; occurrences inside quotes and
//!   `${...}` forms are left alone, because the shell expands them from the
//!   environment the runner exports ([`Vars::export_env`]).
//!
//! [`resolve_command`] picks automatically.
//!
//! # Supported syntax
//!
//! `$name`, `${name}`, `${name:-default}`, `${name%pat}`, `${name%%pat}`,
//! `${name#pat}`, `${name##pat}` (patterns: literal text with `*` and `?`;
//! enough for CodeRunner's `${compiler%:*}` / `${compiler#*:}`), a leading `~`
//! (home), single quotes (literal), double quotes (variables expand), and
//! backslash escapes.
//!
//! ```
//! use std::path::Path;
//! use scratch_language::{expand_command, Vars};
//!
//! let vars = Vars::for_source(Path::new("/work"), "my file.py");
//! let argv = expand_command("python3 $filename", &vars).unwrap();
//! assert_eq!(argv, ["python3", "my file.py"]); // one argument, no quoting needed
//! ```

use crate::error::ExpandError;
use crate::vars::{Value, Vars, shell_quote};

/// A run command ready to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCommand {
    /// Execute this argv directly (the first element is the program).
    Argv(Vec<String>),
    /// Execute this string with `<shell> [-l] -c`.
    Shell(String),
}

impl ResolvedCommand {
    /// The full argv to spawn, with the user's run `arguments` appended (they
    /// are never subject to shell expansion). For [`ResolvedCommand::Shell`]
    /// the argv is `[shell, "-l"?, "-c", "<cmd> \"$@\"", "sh", args...]`
    /// (CodeRunner builds the command the same way).
    pub fn to_argv(&self, shell: &str, login_shell: bool, args: &[String]) -> Vec<String> {
        match self {
            ResolvedCommand::Argv(argv) => {
                let mut out = argv.clone();
                out.extend(args.iter().cloned());
                out
            }
            ResolvedCommand::Shell(command) => {
                let mut out = vec![shell.to_owned()];
                if login_shell {
                    out.push("-l".to_owned());
                }
                out.push("-c".to_owned());
                if args.is_empty() {
                    out.push(command.clone());
                } else {
                    out.push(format!("{command} \"$@\""));
                    out.push("sh".to_owned());
                    out.extend(args.iter().cloned());
                }
                out
            }
        }
    }

    /// The program that will be looked up (`argv[0]` or the shell command's
    /// first word), for "command not found" messages.
    pub fn program(&self) -> Option<String> {
        match self {
            ResolvedCommand::Argv(argv) => argv.first().cloned(),
            ResolvedCommand::Shell(command) => command
                .split_whitespace()
                .find(|word| !word.contains('='))
                .map(str::to_owned),
        }
    }
}

/// Expands `template` into an argv without involving a shell. See the module
/// documentation for the rules.
///
/// # Errors
///
/// [`ExpandError::NeedsShell`] when the template contains shell syntax,
/// [`ExpandError::EmptyCommand`] when it expands to nothing, and the quoting /
/// `${}` errors.
pub fn expand_command(template: &str, vars: &Vars) -> Result<Vec<String>, ExpandError> {
    if let Some(reason) = needs_shell(template) {
        return Err(ExpandError::NeedsShell(reason));
    }
    let words = expand_words(template, vars)?;
    if words.is_empty() {
        return Err(ExpandError::EmptyCommand);
    }
    Ok(words)
}

/// Like [`expand_command`] but a template with no words is not an error (the
/// result is empty). Used for argument lists (`args` of a build step).
pub fn expand_words(template: &str, vars: &Vars) -> Result<Vec<String>, ExpandError> {
    let chars: Vec<char> = template.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
                i += 1;
            }
            '\'' => {
                started = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(&c) => {
                            cur.push(c);
                            i += 1;
                        }
                        None => return Err(ExpandError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                started = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some('\\') => match chars.get(i + 1) {
                            Some(&next @ ('"' | '\\' | '$' | '`')) => {
                                cur.push(next);
                                i += 2;
                            }
                            _ => {
                                cur.push('\\');
                                i += 1;
                            }
                        },
                        Some('$') => {
                            if let Some(value) = parse_dollar(&chars, &mut i, vars)? {
                                cur.push_str(&value.joined());
                            } else {
                                cur.push('$');
                                i += 1;
                            }
                        }
                        Some(&c) => {
                            cur.push(c);
                            i += 1;
                        }
                        None => return Err(ExpandError::UnterminatedQuote),
                    }
                }
            }
            '\\' => match chars.get(i + 1) {
                Some('\n') => {
                    // Line continuation: it does not start a word.
                    i += 2;
                }
                Some(&next) => {
                    started = true;
                    cur.push(next);
                    i += 2;
                }
                None => {
                    started = true;
                    cur.push('\\');
                    i += 1;
                }
            },
            '$' => match parse_dollar(&chars, &mut i, vars)? {
                Some(Value::List(items)) => {
                    let at_boundary = chars.get(i).is_none_or(|c| c.is_whitespace());
                    if !started && at_boundary {
                        words.extend(items);
                    } else {
                        let joined = items.join(" ");
                        if !joined.is_empty() {
                            cur.push_str(&joined);
                            started = true;
                        }
                    }
                }
                Some(Value::Str(s)) => {
                    if !s.is_empty() {
                        cur.push_str(&s);
                        started = true;
                    }
                }
                None => {
                    cur.push('$');
                    started = true;
                    i += 1;
                }
            },
            '~' if !started
                && chars
                    .get(i + 1)
                    .is_none_or(|c| *c == '/' || c.is_whitespace()) =>
            {
                let home = vars
                    .lookup("home")
                    .or_else(|| vars.lookup("HOME"))
                    .map(|v| v.joined())
                    .unwrap_or_default();
                if home.is_empty() {
                    cur.push('~');
                } else {
                    cur.push_str(&home);
                }
                started = true;
                i += 1;
            }
            c => {
                cur.push(c);
                started = true;
                i += 1;
            }
        }
    }
    if started {
        words.push(cur);
    }
    Ok(words)
}

/// Expands one argument template of a build step. Unlike [`expand_command`]
/// the template is *one* word: whitespace, quotes and backslashes in it are
/// literal, only `$name` / `${...}` references are replaced. A list variable
/// (`$flags`) that is the whole argument splices into several arguments; an
/// argument that consists only of references and expands to nothing is
/// dropped.
///
/// ```
/// use std::path::Path;
/// use scratch_language::{expand_arg, Vars};
///
/// let vars = Vars::for_source(Path::new("/w"), "a.go").with_flags("-v -race");
/// assert_eq!(expand_arg("$flags", &vars).unwrap(), ["-v", "-race"]);
/// assert_eq!(expand_arg("-N -l", &vars).unwrap(), ["-N -l"]);
/// assert_eq!(expand_arg("--out=$stem.bin", &vars).unwrap(), ["--out=a.bin"]);
/// ```
pub fn expand_arg(arg: &str, vars: &Vars) -> Result<Vec<String>, ExpandError> {
    let chars: Vec<char> = arg.chars().collect();
    let mut out = String::new();
    let mut literal = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let start = i;
            match parse_dollar(&chars, &mut i, vars)? {
                Some(Value::List(items)) => {
                    if start == 0 && i == chars.len() {
                        return Ok(items);
                    }
                    out.push_str(&items.join(" "));
                }
                Some(Value::Str(s)) => out.push_str(&s),
                None => {
                    out.push('$');
                    literal = true;
                    i += 1;
                }
            }
        } else {
            out.push(chars[i]);
            literal = true;
            i += 1;
        }
    }
    if out.is_empty() && !literal && !arg.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![out])
}

/// Expands a template to a single string (variables replaced, everything else
/// literal; lists are joined with spaces). Used for working directories,
/// environment values and produced paths.
pub fn expand_string(template: &str, vars: &Vars) -> Result<String, ExpandError> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            match parse_dollar(&chars, &mut i, vars)? {
                Some(value) => out.push_str(&value.joined()),
                None => {
                    out.push('$');
                    i += 1;
                }
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// Reads a `$...` reference starting at `chars[*i]` (which is `$`). On success
/// advances `*i` past it and returns the value; returns `None` (and leaves
/// `*i`) when the `$` is just a literal dollar.
fn parse_dollar(chars: &[char], i: &mut usize, vars: &Vars) -> Result<Option<Value>, ExpandError> {
    match chars.get(*i + 1) {
        Some('{') => {
            let start = *i + 2;
            let end = chars[start..]
                .iter()
                .position(|c| *c == '}')
                .map(|p| start + p)
                .ok_or(ExpandError::UnterminatedBrace)?;
            let content: String = chars[start..end].iter().collect();
            *i = end + 1;
            Ok(Some(eval_parameter(&content, vars)?))
        }
        Some(&c) if c.is_ascii_alphabetic() || c == '_' => {
            let mut j = *i + 1;
            while chars
                .get(j)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                j += 1;
            }
            let name: String = chars[*i + 1..j].iter().collect();
            *i = j;
            Ok(Some(
                vars.lookup(&name)
                    .unwrap_or_else(|| Value::Str(String::new())),
            ))
        }
        _ => Ok(None),
    }
}

fn eval_parameter(content: &str, vars: &Vars) -> Result<Value, ExpandError> {
    let name_len = content
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(content.len());
    let (name, rest) = content.split_at(name_len);
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(ExpandError::UnsupportedExpansion(content.to_owned()));
    }
    let value = vars.lookup(name);
    if rest.is_empty() {
        return Ok(value.unwrap_or_else(|| Value::Str(String::new())));
    }
    let text = value.as_ref().map(Value::joined).unwrap_or_default();
    let unset_or_empty = text.is_empty();
    let result = if let Some(default) = rest.strip_prefix(":-") {
        if unset_or_empty {
            default.to_owned()
        } else {
            text
        }
    } else if let Some(default) = rest.strip_prefix('-') {
        if value.is_none() {
            default.to_owned()
        } else {
            text
        }
    } else if let Some(alt) = rest.strip_prefix(":+") {
        if unset_or_empty {
            String::new()
        } else {
            alt.to_owned()
        }
    } else if let Some(pattern) = rest.strip_prefix("%%") {
        remove_suffix(&text, pattern, true)
    } else if let Some(pattern) = rest.strip_prefix('%') {
        remove_suffix(&text, pattern, false)
    } else if let Some(pattern) = rest.strip_prefix("##") {
        remove_prefix(&text, pattern, true)
    } else if let Some(pattern) = rest.strip_prefix('#') {
        remove_prefix(&text, pattern, false)
    } else {
        return Err(ExpandError::UnsupportedExpansion(content.to_owned()));
    };
    Ok(Value::Str(result))
}

fn remove_suffix(text: &str, pattern: &str, longest: bool) -> String {
    let pat: Vec<char> = pattern.chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let starts: Box<dyn Iterator<Item = usize>> = if longest {
        Box::new(0..=chars.len())
    } else {
        Box::new((0..=chars.len()).rev())
    };
    for start in starts {
        if glob_match(&pat, &chars[start..]) {
            return chars[..start].iter().collect();
        }
    }
    text.to_owned()
}

fn remove_prefix(text: &str, pattern: &str, longest: bool) -> String {
    let pat: Vec<char> = pattern.chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let ends: Box<dyn Iterator<Item = usize>> = if longest {
        Box::new((0..=chars.len()).rev())
    } else {
        Box::new(0..=chars.len())
    };
    for end in ends {
        if glob_match(&pat, &chars[..end]) {
            return chars[end..].iter().collect();
        }
    }
    text.to_owned()
}

/// Whole-string glob match with `*` and `?`.
pub(crate) fn glob_match(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| glob_match(rest, &text[skip..])),
        Some(('?', rest)) => !text.is_empty() && glob_match(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob_match(rest, &text[1..]),
    }
}

/// Whether `template` uses shell syntax that cannot be run as a plain argv,
/// and which construct it is.
pub fn needs_shell(template: &str) -> Option<&'static str> {
    #[derive(Clone, Copy, PartialEq)]
    enum Q {
        None,
        Single,
        Double,
    }
    let trimmed = template.trim_start();
    let first_word_end = trimmed
        .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
        .unwrap_or(trimmed.len());
    if let Some(eq) = trimmed[..first_word_end].find('=') {
        let name = &trimmed[..eq];
        if !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Some("a leading NAME=value environment assignment");
        }
    }
    let chars: Vec<char> = template.chars().collect();
    let mut quote = Q::None;
    let mut at_word_start = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Q::Single => {
                if c == '\'' {
                    quote = Q::None;
                }
            }
            Q::Double => match c {
                '"' => quote = Q::None,
                '\\' => i += 1,
                '`' => return Some("command substitution"),
                '$' if chars.get(i + 1) == Some(&'(') => return Some("command substitution"),
                '$' if chars.get(i + 1) == Some(&'{') => {
                    if let Some(p) = chars[i..].iter().position(|c| *c == '}') {
                        i += p;
                    }
                }
                _ => {}
            },
            Q::None => match c {
                '\'' => {
                    quote = Q::Single;
                    at_word_start = false;
                }
                '"' => {
                    quote = Q::Double;
                    at_word_start = false;
                }
                '\\' => {
                    i += 1;
                    at_word_start = false;
                }
                ';' => return Some("a `;` command separator"),
                '|' => return Some("a pipe"),
                '&' => return Some("`&` or `&&`"),
                '<' | '>' => return Some("a redirection"),
                '(' | ')' => return Some("a subshell"),
                '`' => return Some("command substitution"),
                '\n' => return Some("a newline"),
                '#' if at_word_start => return Some("a comment"),
                '$' if chars.get(i + 1) == Some(&'(') => return Some("command substitution"),
                '$' if chars.get(i + 1) == Some(&'{') => {
                    if let Some(p) = chars[i..].iter().position(|c| *c == '}') {
                        i += p;
                    }
                    at_word_start = false;
                }
                '*' | '?' | '[' => return Some("a glob pattern"),
                c if c.is_whitespace() => at_word_start = true,
                _ => at_word_start = false,
            },
        }
        if !matches!(quote, Q::None) {
            at_word_start = false;
        }
        i += 1;
    }
    None
}

/// Expands `template` for execution by a shell (`sh -c`): bare `$name` /
/// `$name` tokens (outside quotes and outside `${...}`) of variables that
/// [`Vars`] holds are replaced by their single-quoted, escaped value. Lists
/// become several quoted words. Everything else is left to the shell, which
/// sees the exported environment ([`Vars::export_env`]).
///
/// ```
/// use std::path::Path;
/// use scratch_language::{expand_shell_command, Vars};
///
/// let vars = Vars::for_source(Path::new("/w"), "it's.css");
/// let cmd = expand_shell_command(r#"echo "<style>"; cat $filename; echo "</style>""#, &vars).unwrap();
/// assert_eq!(cmd, r#"echo "<style>"; cat 'it'\''s.css'; echo "</style>""#);
/// ```
pub fn expand_shell_command(template: &str, vars: &Vars) -> Result<String, ExpandError> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len() + 16);
    let mut single = false;
    let mut double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if single {
            out.push(c);
            if c == '\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                if let Some(&next) = chars.get(i + 1) {
                    out.push(next);
                }
                i += 2;
            }
            '\'' if !double => {
                single = true;
                out.push(c);
                i += 1;
            }
            '"' => {
                double = !double;
                out.push(c);
                i += 1;
            }
            '$' if !double
                && chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_') =>
            {
                let mut j = i + 1;
                while chars
                    .get(j)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    j += 1;
                }
                let name: String = chars[i + 1..j].iter().collect();
                match vars.get(&name) {
                    Some(Value::Str(s)) => out.push_str(&shell_quote(s)),
                    Some(Value::List(items)) => {
                        let quoted: Vec<String> = items.iter().map(|s| shell_quote(s)).collect();
                        out.push_str(&quoted.join(" "));
                    }
                    None => out.extend(chars[i..j].iter()),
                }
                i = j;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    if single || double {
        return Err(ExpandError::UnterminatedQuote);
    }
    Ok(out)
}

/// Expands `template` the best way available: as a direct argv when it can be,
/// through a shell when it uses shell syntax.
pub fn resolve_command(template: &str, vars: &Vars) -> Result<ResolvedCommand, ExpandError> {
    match expand_command(template, vars) {
        Ok(argv) => Ok(ResolvedCommand::Argv(argv)),
        Err(ExpandError::NeedsShell(_)) => {
            expand_shell_command(template, vars).map(ResolvedCommand::Shell)
        }
        Err(other) => Err(other),
    }
}

/// The names of the variables a template references (`$name`, `${name...}`,
/// outside single quotes), in order of first appearance, without duplicates.
pub fn referenced_variables(template: &str) -> Vec<String> {
    let chars: Vec<char> = template.chars().collect();
    let mut names: Vec<String> = Vec::new();
    let mut single = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if single {
            single = c != '\'';
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                single = true;
                i += 1;
            }
            '\\' => i += 2,
            '$' => {
                let start = match chars.get(i + 1) {
                    Some('{') => i + 2,
                    Some(c) if c.is_ascii_alphabetic() || *c == '_' => i + 1,
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                let mut j = start;
                while chars
                    .get(j)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    j += 1;
                }
                let name: String = chars[start..j].iter().collect();
                if !name.is_empty() && !names.contains(&name) {
                    names.push(name);
                }
                i = j.max(i + 1);
            }
            _ => i += 1,
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn vars() -> Vars {
        let mut vars = Vars::for_source(Path::new("/work/proj"), "my file.py")
            .with_flags("-O2 -DNAME=\"a b\"");
        vars.set_compiler("/tmp/out dir/prog");
        vars
    }

    fn argv(t: &str) -> Vec<String> {
        expand_command(t, &vars()).unwrap()
    }

    #[test]
    fn plain_words() {
        assert_eq!(argv("python3 $filename"), ["python3", "my file.py"]);
        assert_eq!(argv("  node   $filename  "), ["node", "my file.py"]);
        assert_eq!(argv("$compiler"), ["/tmp/out dir/prog"]);
    }

    #[test]
    fn values_are_never_resplit_or_reinterpreted() {
        let mut vars = Vars::new();
        vars.set("filename", "a b; rm -rf ~ $(x) `y` *.txt");
        let out = expand_command("cat $filename", &vars).unwrap();
        assert_eq!(out, ["cat", "a b; rm -rf ~ $(x) `y` *.txt"]);
    }

    #[test]
    fn single_quotes_are_literal() {
        assert_eq!(argv("echo '$filename'"), ["echo", "$filename"]);
        assert_eq!(argv(r#"echo 'it'"'"'s'"#), ["echo", "it's"]);
    }

    #[test]
    fn double_quotes_expand_and_group() {
        assert_eq!(argv(r#"echo "x $filename y""#), ["echo", "x my file.py y"]);
        assert_eq!(argv(r#"echo "a\"b\$c""#), ["echo", "a\"b$c"]);
        assert_eq!(
            argv(r#"echo "<script src=\"$file_url\"></script>""#),
            [
                "echo",
                "<script src=\"file:///work/proj/my%20file.py\"></script>"
            ]
        );
    }

    #[test]
    fn backslash_escapes_outside_quotes() {
        assert_eq!(argv(r"echo a\ b"), ["echo", "a b"]);
        assert_eq!(argv(r"echo \$filename"), ["echo", "$filename"]);
    }

    #[test]
    fn list_variables_splice_when_alone_in_a_word() {
        assert_eq!(
            argv("cc $flags -o out"),
            ["cc", "-O2", "-DNAME=a b", "-o", "out"]
        );
        // quoted or embedded lists are joined
        assert_eq!(argv(r#"cc "$flags""#), ["cc", "-O2 -DNAME=a b"]);
        assert_eq!(argv("cc x$flags"), ["cc", "x-O2 -DNAME=a b"]);
    }

    #[test]
    fn empty_unquoted_expansion_drops_the_word() {
        assert_eq!(argv("cc $nothing -o out"), ["cc", "-o", "out"]);
        assert_eq!(argv(r#"cc "$nothing" -o"#), ["cc", "", "-o"]);
        let mut v = Vars::new();
        v.set("flags", Value::List(vec![]));
        assert_eq!(expand_command("cc $flags x", &v).unwrap(), ["cc", "x"]);
    }

    #[test]
    fn parameter_expansion_subset() {
        let mut v = Vars::new();
        v.set("compiler", "/tmp/dir:pkg.Main");
        assert_eq!(
            expand_command(r#"java -cp "${compiler%:*}" "${compiler#*:}""#, &v).unwrap(),
            ["java", "-cp", "/tmp/dir", "pkg.Main"]
        );
        v.set("path", "a/b/c.tar.gz");
        assert_eq!(
            expand_command("x ${path%.*}", &v).unwrap(),
            ["x", "a/b/c.tar"]
        );
        assert_eq!(expand_command("x ${path%%.*}", &v).unwrap(), ["x", "a/b/c"]);
        assert_eq!(
            expand_command("x ${path##*/}", &v).unwrap(),
            ["x", "c.tar.gz"]
        );
        assert_eq!(
            expand_command("x ${path#*/}", &v).unwrap(),
            ["x", "b/c.tar.gz"]
        );
        assert_eq!(
            expand_command("x ${missing:-def}", &v).unwrap(),
            ["x", "def"]
        );
        assert_eq!(
            expand_command("x ${path:-def}", &v).unwrap(),
            ["x", "a/b/c.tar.gz"]
        );
        assert_eq!(expand_command("x ${path:+yes}", &v).unwrap(), ["x", "yes"]);
        assert!(matches!(
            expand_command("x ${path/a/b}", &v),
            Err(ExpandError::UnsupportedExpansion(_))
        ));
        assert_eq!(
            expand_command("x ${path", &v),
            Err(ExpandError::UnterminatedBrace)
        );
    }

    #[test]
    fn braces_delimit_names() {
        assert_eq!(argv("kotlinc ${output}.jar")[1], "/work/proj/my file.jar");
        // `.` already ends a plain name
        assert_eq!(argv("k $output.jar")[1], "/work/proj/my file.jar");
    }

    #[test]
    fn env_fallback_and_literal_dollar() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("HOME".into(), "/Users/me".into());
        let v = Vars::new().with_env(env);
        assert_eq!(
            expand_command("ls ~/x $HOME $ 5$", &v).unwrap(),
            ["ls", "/Users/me/x", "/Users/me", "$", "5$"]
        );
        assert_eq!(expand_command("echo ~root", &v).unwrap(), ["echo", "~root"]);
    }

    #[test]
    fn shell_syntax_is_refused() {
        for (template, ok) in [
            ("a; b", false),
            ("a | b", false),
            ("a && b", false),
            ("a > out", false),
            ("a < in", false),
            ("a $(b)", false),
            ("a `b`", false),
            ("echo *.c", false),
            ("echo [ab]", false),
            ("A=1 cmd", false),
            ("# c", false),
            ("a\nb", false),
            (r#"echo "a;b|c<d>" 'x;y' \;"#, true),
            ("echo a#b", true),
            (r#"echo "${compiler%:*}""#, true),
            ("PATH_X cmd", true),
            ("echo =x", true),
        ] {
            assert_eq!(
                needs_shell(template).is_none(),
                ok,
                "{template:?} -> {:?}",
                needs_shell(template)
            );
        }
        assert!(matches!(
            expand_command("a; b", &Vars::new()),
            Err(ExpandError::NeedsShell(_))
        ));
        assert!(matches!(
            expand_command(r#"echo "$(date)""#, &Vars::new()),
            Err(ExpandError::NeedsShell(_))
        ));
    }

    #[test]
    fn empty_and_unterminated() {
        assert_eq!(
            expand_command("   ", &Vars::new()),
            Err(ExpandError::EmptyCommand)
        );
        assert_eq!(
            expand_command("$nothing", &Vars::new()),
            Err(ExpandError::EmptyCommand)
        );
        assert_eq!(
            expand_command("echo 'a", &Vars::new()),
            Err(ExpandError::UnterminatedQuote)
        );
        assert_eq!(
            expand_command("echo \"a", &Vars::new()),
            Err(ExpandError::UnterminatedQuote)
        );
    }

    #[test]
    fn shell_expansion_quotes_bare_variables_only() {
        let mut vars = Vars::for_source(Path::new("/w"), "it's.css");
        vars.set_compiler("/tmp/a b");
        let t = r#"echo "<style>"; cat $filename; echo "$filename" '$filename' ${filename} $compiler $HOME"#;
        let out = expand_shell_command(t, &vars).unwrap();
        assert_eq!(
            out,
            r#"echo "<style>"; cat 'it'\''s.css'; echo "$filename" '$filename' ${filename} '/tmp/a b' $HOME"#
        );
    }

    #[test]
    fn shell_expansion_leaves_unset_variables_alone() {
        let out = expand_shell_command("tool $compiler", &Vars::new()).unwrap();
        assert_eq!(out, "tool $compiler");
    }

    #[test]
    fn shell_expansion_lists_become_quoted_words() {
        let out = expand_shell_command("cc $flags", &vars()).unwrap();
        assert_eq!(out, r#"cc '-O2' '-DNAME=a b'"#);
    }

    #[test]
    fn shell_expansion_rejects_unterminated_quotes() {
        assert_eq!(
            expand_shell_command("echo 'a", &Vars::new()),
            Err(ExpandError::UnterminatedQuote)
        );
    }

    #[test]
    fn resolve_picks_the_right_mode() {
        let v = vars();
        assert_eq!(
            resolve_command("python3 $filename", &v).unwrap(),
            ResolvedCommand::Argv(vec!["python3".into(), "my file.py".into()])
        );
        assert_eq!(
            resolve_command("cat $filename | wc -l", &v).unwrap(),
            ResolvedCommand::Shell("cat 'my file.py' | wc -l".into())
        );
    }

    #[test]
    fn to_argv_appends_arguments() {
        let args = vec!["a b".to_owned(), "$HOME".to_owned()];
        assert_eq!(
            ResolvedCommand::Argv(vec!["prog".into()]).to_argv("/bin/bash", false, &args),
            ["prog", "a b", "$HOME"]
        );
        assert_eq!(
            ResolvedCommand::Shell("a; b".into()).to_argv("/bin/bash", true, &args),
            ["/bin/bash", "-l", "-c", "a; b \"$@\"", "sh", "a b", "$HOME"]
        );
        assert_eq!(
            ResolvedCommand::Shell("a; b".into()).to_argv("/bin/zsh", false, &[]),
            ["/bin/zsh", "-c", "a; b"]
        );
    }

    #[test]
    fn program_name() {
        assert_eq!(
            ResolvedCommand::Argv(vec!["node".into(), "x".into()])
                .program()
                .as_deref(),
            Some("node")
        );
        assert_eq!(
            ResolvedCommand::Shell("FOO=1 lua 'x'".into())
                .program()
                .as_deref(),
            Some("lua")
        );
    }

    #[test]
    fn referenced_variables_ignores_single_quotes() {
        assert_eq!(
            referenced_variables(r#"a $filename "${compiler%:*}" '$no' $filename $ x"#),
            ["filename", "compiler"]
        );
    }

    #[test]
    fn crlf_and_continuations() {
        assert_eq!(
            expand_command("a \\\n b", &Vars::new()).unwrap(),
            ["a", "b"]
        );
    }

    #[test]
    fn expand_arg_is_one_word() {
        let v = vars();
        assert_eq!(expand_arg("-N -l", &v).unwrap(), ["-N -l"]);
        assert_eq!(expand_arg("$flags", &v).unwrap(), ["-O2", "-DNAME=a b"]);
        assert_eq!(expand_arg("x=$flags", &v).unwrap(), ["x=-O2 -DNAME=a b"]);
        assert_eq!(expand_arg("$nothing", &v).unwrap(), Vec::<String>::new());
        assert_eq!(expand_arg("", &v).unwrap(), [""]);
        assert_eq!(expand_arg("a $filename", &v).unwrap(), ["a my file.py"]);
        assert_eq!(expand_arg("'$filename'", &v).unwrap(), ["'my file.py'"]);
        assert_eq!(expand_arg("cost: $5", &v).unwrap(), ["cost: $5"]);
    }

    #[test]
    fn expand_string_joins() {
        assert_eq!(
            expand_string("$dir/x $flags", &vars()).unwrap(),
            "/work/proj/x -O2 -DNAME=a b"
        );
    }
}
