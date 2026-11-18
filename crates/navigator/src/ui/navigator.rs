//! The file navigator: a flattened `virtual_list` of rows.

use std::{cell::RefCell, collections::HashMap, f32::consts::FRAC_PI_2};

use nsh_base::input;
use nsh_core::{
    element::ImageSrc,
    element::{ClaimRequest, ResponderEvent, TouchClaim},
    keymap::When,
    prelude::*,
};

use super::{
    menus,
    store::{ROW_H, Store},
    theme,
};
use crate::{
    icons::{self, Glyph, IconSpec},
    model::{ClickMods, Disclosure, Kind, fsops, key_bindings},
};

/// Indentation per depth level.
pub const INDENT: f32 = 14.0;

thread_local! {
    static GLYPHS: RefCell<HashMap<Glyph, ImageSrc>> = RefCell::new(HashMap::new());
}

/// The cached `ImageSrc` of a glyph (parsed once per source by NSH).
pub fn glyph_src(glyph: Glyph) -> ImageSrc {
    GLYPHS.with(|g| {
        g.borrow_mut()
            .entry(glyph)
            .or_insert_with(|| ImageSrc::svg_markup(&icons::svg_markup(glyph)))
            .clone()
    })
}

fn chevron_src() -> ImageSrc {
    thread_local! { static C: ImageSrc = ImageSrc::svg_markup(icons::CHEVRON); }
    C.with(|c| c.clone())
}

fn transparent() -> AdaptiveColor {
    theme::fg().opacity(0.0)
}

/// What a cell shows for its current row, derived once per cell.
#[derive(Clone, Copy)]
struct Cell {
    id: Memo<u64>,
    depth: Memo<u16>,
    disclosure: Memo<Disclosure>,
    draft: Memo<bool>,
    icon: Memo<IconSpec>,
    name: Memo<SharedString>,
    present: Memo<bool>,
    selected: Memo<bool>,
    current: Memo<bool>,
    drop: Memo<bool>,
}

fn cell_state(store: Store, at: Position) -> Cell {
    let row = computed(move || store.rows.with(|r| r.0.get(at.index()).cloned()));
    let id = computed(move || row.with(|r| r.as_ref().map_or(0, |r| r.entry.id)));
    Cell {
        id,
        depth: computed(move || row.with(|r| r.as_ref().map_or(0, |r| r.depth))),
        disclosure: computed(move || {
            row.with(|r| r.as_ref().map_or(Disclosure::None, |r| r.disclosure))
        }),
        draft: computed(move || row.with(|r| r.as_ref().is_some_and(|r| r.draft))),
        icon: computed(move || {
            row.with(|r| match r {
                Some(r) if r.entry.is_dir() => icons::FOLDER,
                Some(r) => icons::spec_for_ext(r.entry.extension().as_deref()),
                None => icons::spec_for_ext(None),
            })
        }),
        name: computed(move || {
            row.with(|r| {
                r.as_ref()
                    .map(|r| SharedString::from(r.entry.name.to_string()))
                    .unwrap_or_default()
            })
        }),
        present: computed(move || row.with(Option::is_some)),
        selected: computed(move || {
            let id = id.get();
            store.selection.with(|s| s.contains(id))
        }),
        current: computed(move || store.current_doc.get() == Some(id.get())),
        drop: computed(move || store.drop_target.get() == Some(id.get())),
    }
}

/// One row of the navigator.
fn row_cell(store: Store, at: Position, root: ElementRef) -> impl IntoElement {
    let c = cell_state(store, at);
    let editing = computed(move || store.renaming.get() == Some(c.id.get()) && c.present.get());
    let lit = computed(move || c.selected.get() && store.focused.get());

    let chevron = svg(chevron_src())
        .size(px(scratch_ui_kit::tokens::icon_xs()))
        .flex_shrink(0.0)
        .text_color(move || {
            if lit.get() {
                theme::selection_fg()
            } else {
                theme::muted()
            }
        })
        .opacity(move || match c.disclosure.get() {
            Disclosure::None => 0.0,
            _ => 1.0_f32,
        })
        .rotate(move || {
            Radians(
                if matches!(
                    c.disclosure.get(),
                    Disclosure::Expanded | Disclosure::Loading
                ) {
                    FRAC_PI_2
                } else {
                    0.0
                },
            )
        })
        .transition_transform();

    let icon = svg(move || glyph_src(c.icon.get().glyph))
        .size(px(scratch_ui_kit::tokens::icon_md()))
        .flex_shrink(0.0)
        .text_color(move || {
            if lit.get() {
                theme::selection_fg()
            } else {
                theme::icon_tint(c.icon.get().tint)
            }
        });

    let label = when(editing, move || rename_input(store, c, root)).otherwise(move || {
        text(move || c.name.get())
            .flex_1()
            .min_w(px(0.0))
            .number_of_lines(1)
            .text_size(px(13.0))
            .font_weight(FontWeight::NORMAL)
            .text_color(move || {
                if lit.get() {
                    theme::selection_fg()
                } else {
                    theme::fg()
                }
            })
            .style_when(computed(move || c.current.get()), |s| {
                s.font_weight(FontWeight::SEMIBOLD)
            })
    });

    let inner = view()
        .flex_1()
        .h_full()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .pr(px(6.0))
        .rounded(px(6.0))
        .pl(move || px(4.0 + c.depth.get() as f32 * INDENT))
        .bg(move || {
            if c.selected.get() {
                if store.focused.get() {
                    theme::selection()
                } else {
                    theme::selection_inactive()
                }
            } else if c.drop.get() {
                theme::drop_target()
            } else if c.current.get() {
                theme::current_doc()
            } else {
                transparent()
            }
        })
        .hover(|s| s.bg(theme::row_hover))
        .child(chevron)
        .child(icon)
        .child(label);

    view()
        .h(px(ROW_H))
        .px(px(6.0))
        .flex_row()
        .role(AccessibilityRole::TreeItem)
        .label(move || c.name.get())
        .on_press(move |e: &PressEvent| {
            root.focus();
            let mods = ClickMods {
                shift: e.modifiers.shift,
                cmd: e.modifiers.cmd,
            };
            if c.present.get_untracked() {
                store.press_row(c.id.get_untracked(), mods, e.clicks);
            }
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            root.focus();
            if c.present.get_untracked() && !c.draft.get_untracked() {
                menus::show_navigator_menu(store, Some(c.id.get_untracked()), e.page_position);
            }
        })
        .on_file_drag_enter(move |_: &FileDragEvent| {
            store
                .drop_target
                .set(drop_row_target(store, c.id.get_untracked()))
        })
        .on_file_drag_leave(move || store.drop_target.set(None))
        .on_file_drop(move |e: &FileDropEvent| store.drop_urls(&e.uris, Some(c.id.get_untracked())))
        .child(inner)
}

/// The id highlighted when files are dragged over row `id`: the folder itself, else its parent.
fn drop_row_target(store: Store, id: u64) -> Option<u64> {
    let e = store.tree.with(|t| t.entry(id))?;
    if e.is_dir() {
        Some(e.id)
    } else {
        e.path.parent().map(crate::model::path_id)
    }
}

/// The inline rename / new-item input that replaces a row's label.
fn rename_input(store: Store, c: Cell, root: ElementRef) -> impl IntoElement {
    let name = c.name.get_untracked();
    let is_draft = c.draft.get_untracked();
    let id = c.id.get_untracked();
    let initial = if is_draft {
        if store.rows_now().iter().any(|r| r.draft && r.entry.is_dir()) {
            "untitled folder".to_owned()
        } else {
            "untitled".to_owned()
        }
    } else {
        name.to_string()
    };
    let range = if is_draft {
        (0, initial.encode_utf16().count() as u32)
    } else {
        fsops::stem_range_utf16(&initial)
    };
    let typed = stored(initial.clone());
    let done = move |text: String| {
        store.commit_edit(id, &text);
        root.focus();
    };
    input()
        .test_id("rename-input")
        .default_value(initial)
        .selected_range(range)
        .auto_focus(true)
        .flex_1()
        .min_w(px(0.0))
        .h(px(20.0))
        .px(px(3.0))
        .text_size(px(13.0))
        .rounded(px(3.0))
        .bg(move || theme().background_for_input())
        .border_1()
        .border_color(theme::selection)
        .on_change(move |t: &str| {
            typed.replace(t.to_owned());
        })
        .on_submit(move |t: &str| done(t.to_owned()))
        .on_blur(move || done(typed.get()))
}

trait InputBg {
    fn background_for_input(&self) -> AdaptiveColor;
}
impl InputBg for nsh_base::Theme {
    fn background_for_input(&self) -> AdaptiveColor {
        self.background
    }
}
use nsh_base::theme as theme_fn;
fn theme() -> std::sync::Arc<nsh_base::Theme> {
    theme_fn()
}

/// Binds the Finder-like key model while the list has the keyboard focus.
fn bind_keys(store: Store) {
    effect(move || {
        if !store.focused.get() {
            return;
        }
        for (key, cmd) in key_bindings() {
            let repeat = matches!(
                cmd,
                crate::model::NavCommand::Up { .. }
                    | crate::model::NavCommand::Down { .. }
                    | crate::model::NavCommand::PageUp
                    | crate::model::NavCommand::PageDown
                    | crate::model::NavCommand::Backspace
                    | crate::model::NavCommand::Collapse
                    | crate::model::NavCommand::Expand
            );
            on_key_with(
                &key,
                KeyOptions {
                    when: When::NotEditing,
                    repeat,
                    ..Default::default()
                },
                move |_| store.nav(cmd),
            );
        }
    });
    // Escape cancels an inline edit even while its input has the focus.
    effect(move || {
        if store.renaming.get().is_none() {
            return;
        }
        on_key_with(
            "escape",
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| store.cancel_edit(),
        );
    });
}

/// The drag floater that follows the pointer.
fn drag_floater(store: Store, origin_x: Stored<f32>) -> impl IntoElement {
    when(
        move || store.drag.with(Option::is_some),
        move || {
            view()
                .absolute()
                .pointer_events(PointerEvents::None)
                .left(move || {
                    px(
                        store.drag.with(|d| d.as_ref().map_or(0.0, |d| d.x)) - origin_x.get()
                            + 12.0,
                    )
                })
                .top(move || px(store.drag.with(|d| d.as_ref().map_or(0.0, |d| d.y)) + 6.0))
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .bg(move || theme().popover)
                .border_1()
                .border_color(move || theme().border)
                .shadow_md()
                .child(
                    text(move || {
                        store.drag.with(|d| {
                            d.as_ref()
                                .map(|d| format!("{}{}", if d.copy { "+ " } else { "" }, d.label))
                                .unwrap_or_default()
                        })
                    })
                    .text_size(px(12.0))
                    .text_color(move || theme().popover_foreground),
                )
        },
    )
}

/// The navigator: rows, keyboard, drag and drop. Fills its parent.
pub fn navigator(store: Store) -> impl IntoElement {
    let root = store.root;
    bind_keys(store);

    let list = virtual_list(
        move || store.rows.with(|r| r.0.len()),
        move |at| row_cell(store, at, root),
    )
    .keys(move || {
        store
            .rows
            .with(|r| r.0.iter().map(|r| r.entry.id).collect::<Vec<_>>())
    })
    .estimated_item_size(px(ROW_H))
    .content_padding_y(px(4.0))
    .viewability(Viewability {
        item_visible_threshold: Some(Percentage(0.995)),
        ..Viewability::default()
    })
    .on_viewable_items_change(move |v: &ViewableItems| {
        store.visible.replace(v.viewable);
    })
    .on_scroll(move |e: &ScrollEvent| {
        store.scroll_y.replace(e.offset.y.0);
    })
    .empty(
        view()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .p_4()
            .child(
                text(move || {
                    if store.filter.with(String::is_empty) {
                        "Empty folder"
                    } else {
                        "No Filter Results"
                    }
                })
                .text_size(px(12.0))
                .text_color(theme::muted),
            )
            .child(when(
                move || !store.filter.with(String::is_empty),
                move || super::controls::pill("Reset Filter", move || store.set_filter(String::new())),
            )),
    )
    .element_ref(&store.list)
    .flex_1();

    // Window x of the sidebar's left edge, to place the floater (measured at a drag start).
    let origin_x = stored(0.0_f32);
    let press_origin = stored(None::<(f32, f32)>);
    // Responder events carry no modifiers in the pinned nsh; the Option
    // state for copy-drags is the one the press began with.
    let press_modifiers = stored(nsh_core::keymap::Modifiers::default());

    view()
        .flex_1()
        .min_h(px(0.0))
        .element_ref(&root)
        .role(AccessibilityRole::Tree)
        .label("File navigator")
        .focusable(true)
        .on_focus(move || store.focused.set(true))
        .on_blur(move || store.focused.set(false))
        .on_context_menu(move |e: &ContextMenuEvent| {
            menus::show_navigator_menu(store, None, e.page_position)
        })
        .on_file_drag_enter(move |_: &FileDragEvent| {
            store
                .drop_target
                .set(Some(store.tree.with(|t| t.root().id)))
        })
        .on_file_drag_leave(move || store.drop_target.set(None))
        .on_file_drop(move |e: &FileDropEvent| store.drop_urls(&e.uris, None))
        // In-app drag and drop through the touch responder (what DockArea uses for tabs).
        .claim_touches(TouchClaim::on_move().ask(move |req: &ClaimRequest| {
            let d = req.event.gesture.delta;
            // In a claim question the gesture's origin is where the finger went down; after
            // the grant it is where the grant happened (past the slop), so remember it now.
            press_origin.replace(Some((
                req.event.gesture.origin.x.0,
                req.event.gesture.origin.y.0,
            )));
            if d.x.0.hypot(d.y.0) >= 5.0 && !store.renaming.get_untracked().is_some() {
                req.claim();
            }
        }))
        .keep_touches(true)
        .on_press_down(move |e: &PressEvent| {
            press_modifiers.replace(e.modifiers);
        })
        .on_responder_grant(move |e: &ResponderEvent| {
            // The press started at the gesture origin; measure the list to map y to a row.
            let origin = press_origin
                .get()
                .unwrap_or((e.gesture.origin.x.0, e.gesture.origin.y.0));
            let list = store.list;
            let copy = press_modifiers.get().alt;
            spawn(async move {
                if let Some(m) = list.measure().await {
                    origin_x.replace(m.page.x.0);
                    store.list_origin.replace(Some((
                        m.page.x.0,
                        m.page.y.0,
                        m.frame.size.height.0,
                    )));
                    store.drag_begin(origin, copy);
                }
            });
        })
        .on_responder_move(move |e: &ResponderEvent| {
            store.drag_move((e.page.x.0, e.page.y.0), press_modifiers.get().alt)
        })
        .on_responder_release(move |_: &ResponderEvent| store.drag_end())
        .on_responder_terminate(move |_: &ResponderEvent| store.drag_cancel())
        .child(list)
        .child(drag_floater(store, origin_x))
}

/// New-file/new-folder helper used by the footer buttons.
pub fn new_item(store: Store, kind: Kind) {
    store.begin_new(kind);
}
