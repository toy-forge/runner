//! Shared compact desktop controls, built on NSH's semantic components.

use nsh_base::{SelectEntry, SelectItem, button, select, theme};
use nsh_core::prelude::*;

use super::state::{self, with};

/// A compact command button with hover, press and keyboard focus treatment.
pub fn command(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    button()
        .accessibility_label(label)
        .on_press(action)
        .px_3().py_1().rounded(move || theme().radius)
        .bg(move || theme().secondary)
        .transition_colors().duration(100.0)
        .hover(|s| s.bg(move || theme().accent))
        .active(|s| s.opacity(0.7))
        .focus_visible(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
        .child(text(label).text_sm().text_color(move || theme().foreground))
}

/// A native-behavior selector backed by the loaded language registry.
pub fn language_select() -> impl IntoElement {
    let mut items = with(|workspace| workspace.registry.as_ref().map(|registry| registry.enabled().map(|def| SelectItem::new(def.id.clone(), def.name.clone())).collect::<Vec<_>>()).unwrap_or_default());
    items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    select()
        .items(items)
        .value(move || {
            let view = state::display().get();
            view.active.and_then(|id| view.tabs.iter().find(|tab| tab.id == id).map(|_| SharedString::from(with(|workspace| workspace.doc(id).map(|doc| doc.document.language.clone()).unwrap_or_default()))))
        })
        .on_change(move |chosen: &Option<SharedString>| {
            if let (Some(language), Some(id)) = (chosen, with(|workspace| workspace.active)) { state::set_language(id, language); }
        })
        .accessibility_label("Language")
        .popup_style(|s| s.p_1().rounded(move || theme().radius).border_1().border_color(move || theme().border).bg(move || theme().popover).shadow_md())
        .item_class("rounded-md focus:bg-accent selected:font-medium")
        .render_item(|item: &SelectEntry| view().px_2().py_1().child(text(item.title.clone()).text_sm()))
        .trigger(view().px_3().py_1().rounded(move || theme().radius).border_1().border_color(move || theme().border)
            .child(text(move || {
                let view = state::display().get();
                view.active.and_then(|id| view.tabs.iter().find(|tab| tab.id == id).map(|tab| format!("Language: {} ▾", tab.language))).unwrap_or_else(|| "Language ▾".into())
            }).text_sm()))
}
