//! The navigator's header (path popup, history, search toggle) and footer
//! (new file/folder, go to folder, filter).

use nsh_core::{
    element::{
        ImageSrc,
        desktop::{MenuItem, popup_menu_at},
    },
    prelude::*,
};

use super::{controls, menus, store::Store, theme};
use crate::{
    icons,
    model::{Kind, display_name},
};

/// Header: `[folder name v] .... [<][>] [search]`.
pub fn header(store: Store, on_search: impl Fn() + Copy + 'static) -> impl IntoElement {
    let folder = ImageSrc::svg_markup(&icons::svg_markup(icons::Glyph::Folder));
    let chev = ImageSrc::svg_markup(icons::chrome::CHEVRON_DOWN);
    let root_name = computed(move || store.history.with(|h| display_name(h.current())));
    let path_button = view()
        .role(AccessibilityRole::Button)
        .label("Root folder")
        .flex_1()
        .min_w(px(0.0))
        .h(px(24.0))
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .px(px(6.0))
        .rounded(px(5.0))
        .hover(|s| s.bg(theme::row_hover))
        .active(|s| s.opacity(0.7))
        .on_press(move |e: &PressEvent| {
            // The menu's top-left corner is the button's bottom-left in window coordinates.
            let at = point(
                e.page_position.x - e.position.x,
                e.page_position.y - e.position.y + px(26.0),
            );
            drop(popup_menu_at(menus::path_menu(store), at));
        })
        .child(
            svg(folder)
                .size(px(15.0))
                .text_color(move || theme::icon_tint("icon-folder")),
        )
        .child(
            text(move || root_name.get())
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(1)
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::fg),
        )
        .child(svg(chev).size(px(12.0)).text_color(theme::muted));

    let can_back = computed(move || store.history.with(|h| h.can_back()));
    let can_fwd = computed(move || store.history.with(|h| h.can_forward()));
    let nav_button = move |markup: &'static str, label: &'static str, back: bool| {
        let enabled = if back { can_back } else { can_fwd };
        view()
            .style_when(computed(move || !enabled.get()), |s| s.opacity(0.35))
            .on_long_press(move |e: &PressEvent| {
                let at = point(
                    e.page_position.x - e.position.x,
                    e.page_position.y - e.position.y + px(24.0),
                );
                drop(popup_menu_at(menus::history_menu(store, back), at));
            })
            .child(controls::icon_button(markup, label, move |_| {
                if back { store.back() } else { store.forward() }
            }))
    };

    view()
        .h(px(32.0))
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .px(px(6.0))
        .flex_shrink(0.0)
        .child(path_button)
        .child(nav_button(icons::chrome::BACK, "Back", true))
        .child(nav_button(icons::chrome::FORWARD, "Forward", false))
        .child(controls::icon_button(
            icons::chrome::SEARCH,
            "Search",
            move |_| on_search(),
        ))
}

/// Footer: `[+] [folder+] [go]  [Filter ......]`.
pub fn footer(store: Store) -> impl IntoElement {
    let filter = controls::field("Filter")
        .value(move || store.filter.get())
        .on_change(move |t: &str| store.set_filter(t.to_owned()))
        .flex_1()
        .min_w(px(0.0));
    view()
        .h(px(24.0))
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .px(px(6.0))
        .flex_shrink(0.0)
        .border_t_1()
        .border_t_color(theme::separator)
        .child(
            view()
                .on_context_menu(move |e: &ContextMenuEvent| {
                    let dir = store.root_path();
                    let items = vec![
                        MenuItem::new("New Empty File").on_select(move || store.begin_new(Kind::File)),
                        MenuItem::new("New Folder").on_select(move || store.begin_new(Kind::Dir)),
                        MenuItem::new("New from Active Template…").on_select(move || {
                            if !store.trigger_new_from_template(dir.clone()) {
                                store.begin_new(Kind::File);
                            }
                        }),
                    ];
                    drop(popup_menu_at(items, e.page_position));
                })
                .child(controls::icon_button(icons::chrome::PLUS, "New File", move |_| {
                    store.begin_new(Kind::File)
                })),
        )
        .child(controls::icon_button(
            icons::chrome::FOLDER_PLUS,
            "Create New Folder",
            move |_| store.begin_new(Kind::Dir),
        ))
        .child(controls::icon_button(
            icons::chrome::GOTO,
            "Go to Folder",
            move |_| {
                if !store.trigger_go_to_folder() {
                    store.task(async move {
                        let picked = pick_paths(nsh_core::file_dialog::OpenDialog::folders()).await;
                        if let Ok(Some(v)) = picked
                            && let Some(p) = v.first()
                        {
                            store.go(p);
                        }
                    });
                }
            },
        ))
        .child(filter)
}
