//! The command palette / QuickNav `mainMenu` provider: every menu command with its title and
//! effective shortcut, filtered by a fuzzy query.
//!
//! ```
//! use scratch_commands::palette::{palette_items, PaletteOptions};
//! use scratch_commands::menu::{MenuModel, DefaultShortcuts};
//! use scratch_commands::CommandContext;
//! let menu = MenuModel::standard();
//! let ctx = CommandContext::new();
//! let items = palette_items(&menu, &ctx, &DefaultShortcuts, "save", PaletteOptions::default());
//! assert!(items.iter().any(|i| i.title == "Save As…"));
//! ```

use crate::command::{CommandId, CommandTable};
use crate::context::CommandContext;
use crate::fuzzy::{FuzzyMatch, fuzzy_match};
use crate::menu::{MenuModel, ShortcutSource};

/// What to include in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaletteOptions {
    /// Include disabled commands (shown greyed out).
    pub include_disabled: bool,
    /// Include Option-revealed alternate items as separate rows.
    pub include_alternates: bool,
}

/// One row of the command palette.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteItem {
    /// The command to run.
    pub command: CommandId,
    /// Title for the current context (may be dynamic: `Undo Typing`, `Close Tab` ...).
    pub title: String,
    /// The shortcut, as Mac glyphs (`⇧⌘S`), or empty when the command has none.
    pub subtitle: String,
    /// The menu path with `" > "` separators (`File > Save As…`).
    pub menu_path: String,
    /// Whether the command can be chosen now.
    pub enabled: bool,
    /// Whether the command shows a check mark.
    pub checked: bool,
    /// Fuzzy score (0 when the query is empty).
    pub score: i32,
    /// Character positions of the match inside [`PaletteItem::title`].
    pub positions: Vec<usize>,
}

fn score_of(m: Option<FuzzyMatch>) -> (i32, Vec<usize>) {
    m.map_or((0, Vec::new()), |m| (m.score, m.positions))
}

/// Builds the palette rows for `query`.
///
/// Every static menu-bar item is considered once (alternates only with
/// [`PaletteOptions::include_alternates`]). Rows are sorted by score, highest first; with an
/// empty query the menu-bar order is kept.
pub fn palette_items(
    menu: &MenuModel,
    ctx: &CommandContext,
    shortcuts: &dyn ShortcutSource,
    query: &str,
    options: PaletteOptions,
) -> Vec<PaletteItem> {
    let table = CommandTable::global();
    let mut out = Vec::new();
    for info in menu.items() {
        if info.alternate && !options.include_alternates {
            continue;
        }
        let Some(def) = table.get(&info.command) else { continue };
        let state = def.state(ctx);
        if !state.visible && !info.alternate {
            continue;
        }
        if !state.enabled && !options.include_disabled {
            continue;
        }
        let title = state.title.into_owned();
        let m = fuzzy_match(query, &title);
        if m.is_none() && !query.is_empty() {
            continue;
        }
        let (score, positions) = score_of(m);
        out.push(PaletteItem {
            command: info.command.clone(),
            title,
            subtitle: shortcuts.shortcut_for(&info.command).map(|s| s.display()).unwrap_or_default(),
            menu_path: info.path.join(" > "),
            enabled: state.enabled,
            checked: state.checked,
            score,
            positions,
        });
    }
    if query.is_empty() {
        return out;
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.score));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::DefaultShortcuts;

    #[test]
    fn lists_menu_commands_with_shortcuts() {
        let menu = MenuModel::standard();
        let ctx = CommandContext::new();
        let all = palette_items(&menu, &ctx, &DefaultShortcuts, "", PaletteOptions::default());
        assert!(all.len() > 100, "{}", all.len());
        let save = all.iter().find(|i| i.command == crate::ids::FILE_SAVE).expect("save");
        assert_eq!(save.title, "Save");
        assert_eq!(save.subtitle, "⌘S");
        assert_eq!(save.menu_path, "File > Save");
    }

    #[test]
    fn filters_and_reports_positions() {
        let menu = MenuModel::standard();
        let ctx = CommandContext::new();
        let items = palette_items(&menu, &ctx, &DefaultShortcuts, "quk", PaletteOptions::default());
        assert!(!items.is_empty());
        assert_eq!(items[0].title, "Open Quickly…");
        assert_eq!(items[0].positions, [5, 6, 9]);
    }

    #[test]
    fn dynamic_titles_and_disabled_items() {
        let menu = MenuModel::standard();
        let mut ctx = CommandContext::new();
        ctx.undo_action_name = Some("Typing".into());
        ctx.can_undo = true;
        let items = palette_items(&menu, &ctx, &DefaultShortcuts, "undo", PaletteOptions::default());
        assert!(items.iter().any(|i| i.title == "Undo Typing"));
        let no_can_paste = CommandContext::new();
        let items = palette_items(&menu, &no_can_paste, &DefaultShortcuts, "paste", PaletteOptions::default());
        assert!(!items.iter().any(|i| i.command == crate::ids::EDIT_PASTE));
        let items = palette_items(
            &menu,
            &no_can_paste,
            &DefaultShortcuts,
            "paste",
            PaletteOptions { include_disabled: true, ..PaletteOptions::default() },
        );
        assert!(items.iter().any(|i| i.command == crate::ids::EDIT_PASTE));
    }

    #[test]
    fn alternates_are_opt_in() {
        let menu = MenuModel::standard();
        let ctx = CommandContext::new();
        let without = palette_items(&menu, &ctx, &DefaultShortcuts, "clipboard", PaletteOptions::default());
        assert!(!without.iter().any(|i| i.command == crate::ids::FILE_NEW_FROM_CLIPBOARD_ALT));
        let with = palette_items(
            &menu,
            &ctx,
            &DefaultShortcuts,
            "clipboard",
            PaletteOptions { include_alternates: true, ..PaletteOptions::default() },
        );
        assert!(with.iter().any(|i| i.command == crate::ids::FILE_NEW_FROM_CLIPBOARD_ALT));
    }
}
