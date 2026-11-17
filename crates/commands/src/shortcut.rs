//! Keystrokes, shortcuts (stroke sequences) and their textual forms.
//!
//! One [`Stroke`] is a set of [`Modifiers`] plus a [`Key`]. The key is always
//! the *base* key of the US layout (`8`, not `*`; Shift is a separate flag),
//! which is the model NSH uses, so conversion to NSH strings is lossless.
//!
//! Formats:
//!
//! | Format | Example (Shift+Cmd+S, then `h`) | Functions |
//! |---|---|---|
//! | CodeRunner "key equivalent" (`@ ~ ^ $` prefixes, U+F8F0 between strokes) | `@S` | [`Stroke::parse_crbindings`], [`Shortcut::parse_crbindings`], [`Stroke::to_crbindings`] |
//! | NSH keystroke text (`ctrl+alt+shift+cmd+key`) | `shift+cmd+s` | [`Stroke::parse_nsh`], [`Stroke::to_nsh`] (`Display`) |
//! | Our own text form (NSH strokes separated by spaces) | `alt+cmd+e h` | `FromStr` / `Display` of [`Shortcut`] |
//! | Mac glyphs | `⇧⌘S` | [`Stroke::display`], [`Shortcut::display`] |

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// U+F8F0, the separator between the strokes of a CodeRunner key sequence.
pub const SEQUENCE_SEPARATOR: char = '\u{F8F0}';

/// Why a shortcut string could not be parsed or used.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShortcutError {
    /// The string (or one stroke of it) is empty.
    #[error("empty key equivalent")]
    Empty,
    /// More than one key character follows the modifiers.
    #[error("key equivalent {0:?} has more than one key")]
    TooManyKeys(String),
    /// A textual NSH keystroke could not be parsed.
    #[error("invalid keystroke {text:?}: {reason}")]
    Invalid {
        /// The offending text.
        text: String,
        /// Why it is invalid.
        reason: String,
    },
    /// The shortcut cannot be used as an NSH keystroke.
    #[error("not usable as an NSH shortcut: {0}")]
    Nsh(NshError),
}

/// Why a [`Shortcut`] cannot be handed to NSH (`MenuItem::shortcut`, `on_key`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NshError {
    /// NSH binds one keystroke; the shortcut is a chord of several.
    #[error("key sequences (chords) are not supported by NSH; handle them in the key map")]
    Chord,
    /// The key has no NSH name (function-key Unicode such as Begin or Pause).
    #[error("key {0:?} has no NSH name")]
    UnsupportedKey(char),
    /// The key character is not typed without Shift on the US layout.
    #[error("{0:?} is not a base key of the US layout")]
    NotBaseKey(char),
    /// The function key is outside `f1..=f24`.
    #[error("function key F{0} is outside F1..F24")]
    FunctionKey(u8),
    /// The platform keeps the keystroke for itself (quit, hide, minimize, full screen).
    #[error("reserved by the platform: {0}")]
    Reserved(&'static str),
}

/// Modifier keys of a stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Modifiers {
    /// Control (`^` in CodeRunner strings, ⌃).
    pub ctrl: bool,
    /// Option (`~`, ⌥).
    pub alt: bool,
    /// Shift (`$`, ⇧).
    pub shift: bool,
    /// Command (`@`, ⌘).
    pub cmd: bool,
}

impl Modifiers {
    /// No modifier at all.
    pub const NONE: Modifiers = Modifiers { ctrl: false, alt: false, shift: false, cmd: false };
    /// Command only.
    pub const CMD: Modifiers = Modifiers { ctrl: false, alt: false, shift: false, cmd: true };

    /// Whether no modifier is held.
    pub fn is_empty(self) -> bool {
        self == Modifiers::NONE
    }
}

/// A key (without modifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// A character key, named by the unshifted US-layout character (`a`, `1`, `[`, `' '` for Space).
    Char(char),
    /// A function key, `F(1)` is F1.
    F(u8),
    /// Return / Enter.
    Enter,
    /// Escape.
    Escape,
    /// Tab.
    Tab,
    /// Backspace (the key labelled "delete" on Mac keyboards).
    Backspace,
    /// Forward delete.
    Delete,
    /// Insert / Help.
    Insert,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Any other AppKit function-key character (Begin, Pause, Print Screen ...), kept verbatim.
    Special(char),
}

/// The characters typed with Shift on the US layout and the key typing them.
const SHIFTED: &[(char, char)] = &[
    ('~', '`'),
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
];

/// The character typed by `base` with Shift held (`8` gives `*`, `a` gives `A`).
pub fn shifted_char(base: char) -> char {
    SHIFTED
        .iter()
        .find(|(_, b)| *b == base)
        .map_or_else(|| base.to_uppercase().next().unwrap_or(base), |(s, _)| *s)
}

/// Splits a typed character into `(base key character, shift)`.
///
/// `'A'` gives `('a', true)`, `'*'` gives `('8', true)`, `'x'` gives `('x', false)`.
pub fn split_shifted(c: char) -> (char, bool) {
    if let Some((_, base)) = SHIFTED.iter().find(|(s, _)| *s == c) {
        return (*base, true);
    }
    if c.is_uppercase() {
        let mut lower = c.to_lowercase();
        if let (Some(l), None) = (lower.next(), lower.next()) {
            return (l, true);
        }
    }
    (c, false)
}

impl Key {
    /// The key for the character `c` typed on the US layout, plus whether Shift is implied.
    ///
    /// Control characters map to their named keys (`\r`/`\n` to [`Key::Enter`], `\t`, ESC, DEL/BS),
    /// AppKit function-key characters (U+F700..) to arrows, F-keys etc.
    pub fn from_char(c: char) -> (Key, bool) {
        match c {
            '\r' | '\n' | '\u{3}' => (Key::Enter, false),
            '\t' => (Key::Tab, false),
            '\u{1b}' => (Key::Escape, false),
            '\u{7f}' | '\u{8}' => (Key::Backspace, false),
            '\u{F700}' => (Key::Up, false),
            '\u{F701}' => (Key::Down, false),
            '\u{F702}' => (Key::Left, false),
            '\u{F703}' => (Key::Right, false),
            '\u{F704}'..='\u{F726}' => (Key::F((c as u32 - 0xF703) as u8), false),
            '\u{F727}' => (Key::Insert, false),
            '\u{F728}' => (Key::Delete, false),
            '\u{F729}' => (Key::Home, false),
            '\u{F72B}' => (Key::End, false),
            '\u{F72C}' => (Key::PageUp, false),
            '\u{F72D}' => (Key::PageDown, false),
            '\u{F72A}' | '\u{F72E}'..='\u{F8FF}' => (Key::Special(c), false),
            c if c.is_control() => (Key::Special(c), false),
            c => {
                let (base, shift) = split_shifted(c);
                (Key::Char(base), shift)
            }
        }
    }

    /// The AppKit character for this key as written in `.crBindings` (`shift` selects the shifted form of a character key).
    pub fn to_char(self, shift: bool) -> char {
        match self {
            Key::Char(c) if shift => shifted_char(c),
            Key::Char(c) => c,
            Key::F(n) => char::from_u32(0xF703 + u32::from(n)).unwrap_or('\u{F704}'),
            Key::Enter => '\r',
            Key::Escape => '\u{1b}',
            Key::Tab => '\t',
            Key::Backspace => '\u{7f}',
            Key::Delete => '\u{F728}',
            Key::Insert => '\u{F727}',
            Key::Up => '\u{F700}',
            Key::Down => '\u{F701}',
            Key::Left => '\u{F702}',
            Key::Right => '\u{F703}',
            Key::Home => '\u{F729}',
            Key::End => '\u{F72B}',
            Key::PageUp => '\u{F72C}',
            Key::PageDown => '\u{F72D}',
            Key::Special(c) => c,
        }
    }

    /// Whether Shift is already expressed by the shifted character form of this key.
    fn has_shifted_char(self) -> bool {
        matches!(self, Key::Char(_))
    }
}

/// One key press with its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Stroke {
    /// Modifiers held.
    pub modifiers: Modifiers,
    /// The key.
    pub key: Key,
}

/// Named keys of the NSH text form.
const NSH_NAMES: &[(&str, Key)] = &[
    ("enter", Key::Enter),
    ("escape", Key::Escape),
    ("tab", Key::Tab),
    ("space", Key::Char(' ')),
    ("backspace", Key::Backspace),
    ("delete", Key::Delete),
    ("insert", Key::Insert),
    ("up", Key::Up),
    ("down", Key::Down),
    ("left", Key::Left),
    ("right", Key::Right),
    ("home", Key::Home),
    ("end", Key::End),
    ("pageup", Key::PageUp),
    ("pagedown", Key::PageDown),
];

/// Extra spellings accepted when parsing NSH text.
const NSH_ALIASES: &[(&str, Key)] = &[
    ("return", Key::Enter),
    ("esc", Key::Escape),
    ("del", Key::Delete),
    ("arrowup", Key::Up),
    ("arrowdown", Key::Down),
    ("arrowleft", Key::Left),
    ("arrowright", Key::Right),
    ("plus", Key::Char('=')),
    ("minus", Key::Char('-')),
];

fn invalid(text: &str, reason: impl fmt::Display) -> ShortcutError {
    ShortcutError::Invalid { text: text.to_owned(), reason: reason.to_string() }
}

impl Stroke {
    /// A stroke from parts.
    pub const fn new(modifiers: Modifiers, key: Key) -> Stroke {
        Stroke { modifiers, key }
    }

    /// A plain key without modifiers.
    pub const fn plain(key: Key) -> Stroke {
        Stroke { modifiers: Modifiers::NONE, key }
    }

    /// Parses one CodeRunner key equivalent (no sequence separator): optional `@ ~ ^ $`
    /// prefixes in any order, then exactly one key character.
    ///
    /// The last character is always the key, so `^` and `$` alone are keys and `^$` is
    /// Control plus the `$` key. An upper-case letter or a shifted punctuation character
    /// implies Shift, as in AppKit.
    ///
    /// The AppKit "control character" spellings are recognised: `^m`/`^M` is Return, `^h`/`^H`
    /// and `^?` are Backspace and `^[` is Escape (they are what the Vim set uses). Write real
    /// Control-M as `^m` with an extra marker is not possible; the notation wins.
    pub fn parse_crbindings(s: &str) -> Result<Stroke, ShortcutError> {
        match s {
            "^m" | "^M" => return Ok(Stroke::plain(Key::Enter)),
            "^h" | "^H" | "^?" => return Ok(Stroke::plain(Key::Backspace)),
            "^[" => return Ok(Stroke::plain(Key::Escape)),
            _ => {}
        }
        let chars: Vec<char> = s.chars().collect();
        if chars.is_empty() {
            return Err(ShortcutError::Empty);
        }
        let mut m = Modifiers::NONE;
        let mut i = 0;
        while chars.len() - i > 1 {
            match chars[i] {
                '@' => m.cmd = true,
                '~' => m.alt = true,
                '^' => m.ctrl = true,
                '$' => m.shift = true,
                _ => break,
            }
            i += 1;
        }
        if chars.len() - i != 1 {
            return Err(ShortcutError::TooManyKeys(s.to_owned()));
        }
        let (key, shift) = Key::from_char(chars[i]);
        m.shift |= shift;
        Ok(Stroke { modifiers: m, key })
    }

    /// The CodeRunner key equivalent of this stroke (inverse of [`Stroke::parse_crbindings`]).
    ///
    /// Order is `^ ~ $ @`; Shift is folded into the key character for character keys
    /// (`@S`, `@*`) and written as `$` for named keys (`$\r`).
    pub fn to_crbindings(&self) -> String {
        let mut s = String::new();
        let m = self.modifiers;
        if m.ctrl {
            s.push('^');
        }
        if m.alt {
            s.push('~');
        }
        if m.shift && !self.key.has_shifted_char() {
            s.push('$');
        }
        if m.cmd {
            s.push('@');
        }
        s.push(self.key.to_char(m.shift));
        s
    }

    /// Parses one NSH keystroke text (`"mod+shift+n"`, `"f5"`, `"mod+,"`), with NSH's rules:
    /// modifiers `ctrl`, `alt`/`option`, `shift`, `cmd`/`meta`/`super`/`mod` (all Command on
    /// macOS), keys are US base characters, `space`, `enter`, `escape`, `tab`, `backspace`,
    /// `delete`, arrows, `home`, `end`, `pageup`, `pagedown`, `f1..f24`. Shifted characters
    /// (`*`, `{`) are rejected: write `shift+8`. Case is ignored.
    pub fn parse_nsh(text: &str) -> Result<Stroke, ShortcutError> {
        if text.is_empty() {
            return Err(ShortcutError::Empty);
        }
        if text.contains(char::is_whitespace) {
            return Err(invalid(text, "one keystroke per binding"));
        }
        let lower = text.to_ascii_lowercase();
        if lower == "+" || lower.ends_with("++") {
            return Err(invalid(text, "write the plus key as \"plus\""));
        }
        let (mods, key) = match lower.rsplit_once('+') {
            Some((mods, key)) => (mods.split('+').collect::<Vec<_>>(), key),
            None => (Vec::new(), lower.as_str()),
        };
        if key.is_empty() {
            return Err(invalid(text, "no key after the modifiers"));
        }
        let mut m = Modifiers::NONE;
        for name in mods {
            let flag = match name {
                "ctrl" | "control" => &mut m.ctrl,
                "alt" | "option" | "opt" => &mut m.alt,
                "shift" => &mut m.shift,
                "cmd" | "command" | "meta" | "super" | "win" | "mod" | "cmdorctrl" | "commandorcontrol" => &mut m.cmd,
                "" => return Err(invalid(text, "an empty modifier")),
                other => return Err(invalid(text, format_args!("unknown modifier {other:?}"))),
            };
            if *flag {
                return Err(invalid(text, format_args!("{name:?} twice")));
            }
            *flag = true;
        }
        Ok(Stroke { modifiers: m, key: parse_nsh_key(text, key)? })
    }

    /// The NSH text of this stroke, `ctrl+alt+shift+cmd+key` (always canonical, always lowercase).
    ///
    /// This does not check that NSH accepts the keystroke; use [`Stroke::check_nsh`] or
    /// [`Shortcut::to_nsh`] for that.
    pub fn to_nsh(&self) -> String {
        self.to_string()
    }

    /// Checks that NSH accepts this stroke (`MenuItem::shortcut`, `on_key` panic otherwise):
    /// a nameable key, F1..F24, a base US character, and not a platform-reserved keystroke.
    pub fn check_nsh(&self) -> Result<(), NshError> {
        match self.key {
            Key::Special(c) => return Err(NshError::UnsupportedKey(c)),
            Key::F(n) if !(1..=24).contains(&n) => return Err(NshError::FunctionKey(n)),
            Key::Char(c) if SHIFTED.iter().any(|(s, _)| *s == c) || !c.is_ascii_graphic() && c != ' ' || c.is_ascii_uppercase() => {
                return Err(NshError::NotBaseKey(c));
            }
            Key::Char(_) => {}
            _ => {}
        }
        match self.reserved_reason() {
            Some(why) => Err(NshError::Reserved(why)),
            None => Ok(()),
        }
    }

    /// Why macOS keeps this keystroke for itself (NSH refuses to bind it), if it does.
    ///
    /// The reserved keystrokes are Cmd+Q, Cmd+H, Opt+Cmd+H, Cmd+M and Ctrl+Cmd+F.
    pub fn reserved_reason(&self) -> Option<&'static str> {
        let m = self.modifiers;
        let only = |ctrl, alt, cmd| m.ctrl == ctrl && m.alt == alt && m.cmd == cmd && !m.shift;
        match self.key {
            Key::Char('q') if only(false, false, true) => Some("it quits the app"),
            Key::Char('h') if only(false, false, true) => Some("it hides the app"),
            Key::Char('h') if only(false, true, true) => Some("it hides the other apps"),
            Key::Char('m') if only(false, false, true) => Some("it minimizes the window"),
            Key::Char('f') if only(true, false, true) => Some("it toggles full screen"),
            _ => None,
        }
    }

    /// The stroke as Mac menus show it: `⌃⌥⇧⌘S`, arrows as `↑`, Return as `↩`.
    pub fn display(&self) -> String {
        let m = self.modifiers;
        let mut s = String::new();
        for (on, glyph) in [(m.ctrl, '⌃'), (m.alt, '⌥'), (m.shift, '⇧'), (m.cmd, '⌘')] {
            if on {
                s.push(glyph);
            }
        }
        match self.key {
            Key::Char(' ') => s.push_str("Space"),
            Key::Char(c) => s.extend(c.to_uppercase()),
            Key::F(n) => s.push_str(&format!("F{n}")),
            Key::Enter => s.push('↩'),
            Key::Escape => s.push('⎋'),
            Key::Tab => s.push('⇥'),
            Key::Backspace => s.push('⌫'),
            Key::Delete => s.push('⌦'),
            Key::Insert => s.push_str("Help"),
            Key::Up => s.push('↑'),
            Key::Down => s.push('↓'),
            Key::Left => s.push('←'),
            Key::Right => s.push('→'),
            Key::Home => s.push('↖'),
            Key::End => s.push('↘'),
            Key::PageUp => s.push('⇞'),
            Key::PageDown => s.push('⇟'),
            Key::Special(c) => s.push_str(special_name(c)),
        }
        s
    }

    /// The character this key types, if it types one (`shift` applied): used by "typed character" commands.
    pub fn typed_char(&self) -> Option<char> {
        match self.key {
            Key::Char(c) if self.modifiers.shift => Some(shifted_char(c)),
            Key::Char(c) => Some(c),
            Key::Enter => Some('\n'),
            Key::Tab => Some('\t'),
            _ => None,
        }
    }
}

fn special_name(c: char) -> &'static str {
    match c {
        '\u{F72A}' => "Begin",
        '\u{F72E}' => "PrtScr",
        '\u{F72F}' => "ScrLk",
        '\u{F730}' => "Pause",
        '\u{F735}' => "Menu",
        _ => "?",
    }
}

fn parse_nsh_key(text: &str, key: &str) -> Result<Key, ShortcutError> {
    if let Some((_, k)) = NSH_NAMES.iter().chain(NSH_ALIASES).find(|(n, _)| *n == key) {
        return Ok(*k);
    }
    if let Some(n) = key.strip_prefix('f').filter(|n| !n.starts_with('0')).and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Ok(Key::F(n));
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(invalid(text, format_args!("unknown key {key:?}")));
    };
    if let Some((_, base)) = SHIFTED.iter().find(|(s, _)| *s == c) {
        return Err(invalid(text, format_args!("{c:?} is typed with Shift: write \"shift+{base}\"")));
    }
    if !c.is_ascii_graphic() {
        return Err(invalid(text, "keys are named by the US layout's characters"));
    }
    Ok(Key::Char(c))
}

impl fmt::Display for Stroke {
    /// The canonical NSH text: `ctrl+alt+shift+cmd+<key>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [(m.ctrl, "ctrl+"), (m.alt, "alt+"), (m.shift, "shift+"), (m.cmd, "cmd+")] {
            if on {
                f.write_str(name)?;
            }
        }
        match self.key {
            Key::Char(' ') => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "f{n}"),
            Key::Special(c) => write!(f, "special-{:04x}", c as u32),
            key => {
                let name = NSH_NAMES.iter().find(|(_, k)| *k == key).map(|(n, _)| *n);
                f.write_str(name.unwrap_or("?"))
            }
        }
    }
}

impl FromStr for Stroke {
    type Err = ShortcutError;

    /// Parses the NSH text form, plus `special-XXXX` as written by `Display` for exotic keys.
    fn from_str(s: &str) -> Result<Stroke, ShortcutError> {
        if let Some((mods, hex)) = s.rsplit_once("special-").or_else(|| s.strip_prefix("special-").map(|h| ("", h)))
            && let Ok(cp) = u32::from_str_radix(hex, 16)
            && let Some(c) = char::from_u32(cp)
        {
            let base = mods.trim_end_matches('+');
            let mut stroke = if base.is_empty() { Stroke::plain(Key::Enter) } else { Stroke::parse_nsh(&format!("{base}+enter"))? };
            stroke.key = Key::Special(c);
            return Ok(stroke);
        }
        Stroke::parse_nsh(s)
    }
}

/// A shortcut: one stroke, or a chord of several pressed one after the other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Shortcut(Vec<Stroke>);

impl Shortcut {
    /// A single-stroke shortcut.
    pub fn single(stroke: Stroke) -> Shortcut {
        Shortcut(vec![stroke])
    }

    /// A shortcut from strokes; `None` when `strokes` is empty.
    pub fn from_strokes(strokes: Vec<Stroke>) -> Option<Shortcut> {
        (!strokes.is_empty()).then_some(Shortcut(strokes))
    }

    /// The strokes, in press order (never empty).
    pub fn strokes(&self) -> &[Stroke] {
        &self.0
    }

    /// Whether this is a chord of two or more strokes.
    pub fn is_chord(&self) -> bool {
        self.0.len() > 1
    }

    /// Whether `self` starts with all the strokes of `prefix` and is longer than it.
    pub fn has_prefix(&self, prefix: &[Stroke]) -> bool {
        self.0.len() > prefix.len() && self.0.starts_with(prefix)
    }

    /// Parses a CodeRunner key equivalent, strokes separated by U+F8F0.
    pub fn parse_crbindings(s: &str) -> Result<Shortcut, ShortcutError> {
        let strokes = s.split(SEQUENCE_SEPARATOR).map(Stroke::parse_crbindings).collect::<Result<Vec<_>, _>>()?;
        Ok(Shortcut(strokes))
    }

    /// The CodeRunner key equivalent (strokes joined with U+F8F0).
    pub fn to_crbindings(&self) -> String {
        let mut out = String::new();
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(SEQUENCE_SEPARATOR);
            }
            out.push_str(&s.to_crbindings());
        }
        out
    }

    /// The text NSH takes for this shortcut, after checking that NSH would accept it
    /// (single stroke, nameable key, not platform-reserved).
    pub fn to_nsh(&self) -> Result<String, NshError> {
        match self.0.as_slice() {
            [one] => one.check_nsh().map(|()| one.to_nsh()),
            _ => Err(NshError::Chord),
        }
    }

    /// Mac glyph text (`⌥⌘E H` for a chord: strokes are separated by a space).
    pub fn display(&self) -> String {
        self.0.iter().map(Stroke::display).collect::<Vec<_>>().join(" ")
    }
}

impl fmt::Display for Shortcut {
    /// The canonical text form: NSH strokes separated by one space (`alt+cmd+e h`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{s}")?;
        }
        Ok(())
    }
}

impl FromStr for Shortcut {
    type Err = ShortcutError;

    /// Parses the canonical text form (NSH strokes separated by whitespace).
    fn from_str(s: &str) -> Result<Shortcut, ShortcutError> {
        let strokes = s.split_whitespace().map(str::parse::<Stroke>).collect::<Result<Vec<_>, _>>()?;
        Shortcut::from_strokes(strokes).ok_or(ShortcutError::Empty)
    }
}

/// Parses a user filter such as the Key Bindings pane accepts ("`@` for command, `^` for
/// control, `~` for option, `$` for shift"): returns the shortcut when `text` is a valid
/// CodeRunner key equivalent, `None` otherwise (treat it as plain text then).
pub fn parse_filter_shortcut(text: &str) -> Option<Shortcut> {
    if text.is_empty() || !text.chars().any(|c| "@~^$".contains(c)) {
        return None;
    }
    Shortcut::parse_crbindings(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(s: &str) -> Stroke {
        Stroke::parse_crbindings(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    #[test]
    fn parses_crbindings_modifiers() {
        let s = cr("@n");
        assert_eq!(s, Stroke::new(Modifiers::CMD, Key::Char('n')));
        let s = cr("$@S");
        assert!(s.modifiers.shift && s.modifiers.cmd);
        assert_eq!(s.key, Key::Char('s'));
        assert_eq!(cr("@S"), s, "upper-case letter implies shift");
        let s = cr("^~@r");
        assert!(s.modifiers.ctrl && s.modifiers.alt && s.modifiers.cmd && !s.modifiers.shift);
    }

    #[test]
    fn modifier_characters_can_be_keys() {
        assert_eq!(cr("^"), Stroke::new(Modifiers { shift: true, ..Modifiers::NONE }, Key::Char('6')));
        assert_eq!(cr("$"), Stroke::new(Modifiers { shift: true, ..Modifiers::NONE }, Key::Char('4')));
        let s = cr("^$");
        assert!(s.modifiers.ctrl && s.modifiers.shift);
        assert_eq!(s.key, Key::Char('4'));
        assert_eq!(cr("@~"), Stroke::new(Modifiers { cmd: true, shift: true, ..Modifiers::NONE }, Key::Char('`')));
    }

    #[test]
    fn shifted_punctuation_and_named_keys() {
        let s = cr("@*");
        assert_eq!((s.modifiers.cmd, s.modifiers.shift, s.key), (true, true, Key::Char('8')));
        assert_eq!(cr("@+").key, Key::Char('='));
        assert_eq!(cr("\u{1b}"), Stroke::plain(Key::Escape));
        assert_eq!(cr("^ "), Stroke::new(Modifiers { ctrl: true, ..Modifiers::NONE }, Key::Char(' ')));
        assert_eq!(cr("@\n"), cr("@\r"));
        assert_eq!(cr("\u{7f}"), cr("\u{8}"));
        assert_eq!(cr("^~\u{F700}").key, Key::Up);
        assert_eq!(cr("~\u{F729}").key, Key::Home);
        assert_eq!(cr("\u{F70F}").key, Key::F(12));
        assert_eq!(cr("\u{F728}").key, Key::Delete);
    }

    #[test]
    fn control_character_notation() {
        assert_eq!(cr("^m"), Stroke::plain(Key::Enter));
        assert_eq!(cr("^M"), Stroke::plain(Key::Enter));
        assert_eq!(cr("^h"), Stroke::plain(Key::Backspace));
        assert_eq!(cr("^?"), Stroke::plain(Key::Backspace));
        assert_eq!(cr("^["), Stroke::plain(Key::Escape));
        // A real control-Escape stays control-Escape.
        assert_eq!(cr("^\u{1b}"), Stroke::new(Modifiers { ctrl: true, ..Modifiers::NONE }, Key::Escape));
        // ^J is Ctrl+J (the Vim set distinguishes it from Return).
        assert_eq!(cr("^j"), Stroke::new(Modifiers { ctrl: true, ..Modifiers::NONE }, Key::Char('j')));
    }

    #[test]
    fn rejects_bad_crbindings() {
        assert_eq!(Stroke::parse_crbindings(""), Err(ShortcutError::Empty));
        assert!(matches!(Stroke::parse_crbindings("ab"), Err(ShortcutError::TooManyKeys(_))));
        assert!(matches!(Stroke::parse_crbindings("@ab"), Err(ShortcutError::TooManyKeys(_))));
    }

    #[test]
    fn sequences() {
        let s = Shortcut::parse_crbindings("@~e\u{F8F0}h").unwrap();
        assert_eq!(s.strokes().len(), 2);
        assert!(s.is_chord());
        assert_eq!(s.to_string(), "alt+cmd+e h");
        assert_eq!(s.display(), "⌥⌘E H");
        assert_eq!(s.to_nsh(), Err(NshError::Chord));
        assert!(s.has_prefix(&s.strokes()[..1]));
        assert!(!s.has_prefix(s.strokes()));
        assert_eq!("alt+cmd+e h".parse::<Shortcut>().unwrap(), s);
    }

    #[test]
    fn crbindings_roundtrip() {
        for raw in ["@n", "@S", "@*", "^~@r", "$\r", "\t", "$\t", "^~\u{F700}", "\u{1b}", "^ ", "@~\u{F729}", "@~e\u{F8F0}h", "j", "G", "$", "^", "\u{7f}"] {
            let a = Shortcut::parse_crbindings(raw).unwrap();
            let b = Shortcut::parse_crbindings(&a.to_crbindings()).unwrap();
            assert_eq!(a, b, "{raw:?} -> {:?}", a.to_crbindings());
        }
    }

    #[test]
    fn nsh_parse_and_format() {
        let s = Stroke::parse_nsh("mod+shift+n").unwrap();
        assert_eq!(s, cr("$@n"));
        assert_eq!(s.to_nsh(), "shift+cmd+n");
        assert_eq!(Stroke::parse_nsh("Mod+Shift+N").unwrap(), s);
        assert_eq!(Stroke::parse_nsh("f5").unwrap().key, Key::F(5));
        assert_eq!(Stroke::parse_nsh("mod+,").unwrap().key, Key::Char(','));
        assert_eq!(Stroke::parse_nsh("mod+plus").unwrap().key, Key::Char('='));
        assert!(Stroke::parse_nsh("mod+*").is_err(), "NSH rejects shifted characters");
        assert!(Stroke::parse_nsh("mod+shift+").is_err());
        assert!(Stroke::parse_nsh("mod mod+n").is_err());
        assert!(Stroke::parse_nsh("hyper+n").is_err());
        assert!(Stroke::parse_nsh("f25").is_err());
        assert!(Stroke::parse_nsh("shift+shift+a").is_err());
    }

    #[test]
    fn shifted_symbols_convert_to_nsh() {
        let s = cr("@*");
        assert_eq!(s.to_nsh(), "shift+cmd+8");
        assert!(s.check_nsh().is_ok());
        assert_eq!(Stroke::parse_nsh(&s.to_nsh()).unwrap(), s);
    }

    #[test]
    fn nsh_roundtrip_of_all_named_keys() {
        for name in ["enter", "escape", "tab", "space", "backspace", "delete", "insert", "up", "down", "left", "right", "home", "end", "pageup", "pagedown", "f1", "f24", "a", "1", "[", "`"] {
            let s = Stroke::parse_nsh(&format!("ctrl+alt+{name}")).unwrap();
            assert_eq!(Stroke::parse_nsh(&s.to_nsh()).unwrap(), s, "{name}");
            assert!(s.check_nsh().is_ok(), "{name}");
        }
    }

    #[test]
    fn reserved_keys() {
        for r in ["cmd+q", "cmd+h", "alt+cmd+h", "cmd+m", "ctrl+cmd+f"] {
            let s = Stroke::parse_nsh(r).unwrap();
            assert!(s.reserved_reason().is_some(), "{r}");
            assert!(matches!(s.check_nsh(), Err(NshError::Reserved(_))), "{r}");
        }
        for ok in ["cmd+w", "cmd+r", "cmd+,", "shift+cmd+q", "ctrl+q", "cmd+f", "shift+cmd+h"] {
            assert!(Stroke::parse_nsh(ok).unwrap().reserved_reason().is_none(), "{ok}");
        }
    }

    #[test]
    fn nsh_check_rejects_unusable_keys() {
        assert!(matches!(Stroke::plain(Key::Special('\u{F72A}')).check_nsh(), Err(NshError::UnsupportedKey(_))));
        assert!(matches!(Stroke::plain(Key::F(30)).check_nsh(), Err(NshError::FunctionKey(30))));
        assert!(matches!(Stroke::plain(Key::Char('*')).check_nsh(), Err(NshError::NotBaseKey('*'))));
        assert!(matches!(Stroke::plain(Key::Char('A')).check_nsh(), Err(NshError::NotBaseKey('A'))));
    }

    #[test]
    fn glyphs() {
        assert_eq!(cr("$@S").display(), "⇧⌘S");
        assert_eq!(cr("^~@r").display(), "⌃⌥⌘R");
        assert_eq!(cr("@\u{F700}").display(), "⌘↑");
        assert_eq!(cr("\u{1b}").display(), "⎋");
        assert_eq!(cr("$@\u{7f}").display(), "⇧⌘⌫");
        assert_eq!(cr("@*").display(), "⇧⌘8");
        assert_eq!(cr("^ ").display(), "⌃Space");
    }

    #[test]
    fn typed_chars() {
        assert_eq!(cr("a").typed_char(), Some('a'));
        assert_eq!(cr("A").typed_char(), Some('A'));
        assert_eq!(cr("$").typed_char(), Some('$'));
        assert_eq!(cr("\r").typed_char(), Some('\n'));
        assert_eq!(cr("\u{F700}").typed_char(), None);
    }

    #[test]
    fn filter_shortcuts() {
        assert!(parse_filter_shortcut("@s").is_some());
        assert!(parse_filter_shortcut("run").is_none());
        assert!(parse_filter_shortcut("").is_none());
    }

    #[test]
    fn special_keys_roundtrip_text() {
        let s = Stroke::new(Modifiers::CMD, Key::Special('\u{F72A}'));
        assert_eq!(s.to_string().parse::<Stroke>().unwrap(), s);
    }
}
