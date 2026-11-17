//! macOS-native services the custom editor cannot inherit from the text
//! system: the character viewer (Edit > Emoji & Symbols), dictation, speech
//! (Edit > Speech), and rich-text pasteboard writes (Edit > Copy with
//! Colors). Everything degrades to a no-op off macOS.

/// The pasteboard's HTML flavor, written next to the plain text so plain
/// targets still get the text.
#[cfg(target_os = "macos")]
#[allow(deprecated)] // NSSpeechSynthesizer, deprecated in macOS 14, still speaks.
mod imp {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2::rc::Retained;
    use objc2::sel;
    use objc2_app_kit::{NSApplication, NSFont, NSPageLayout, NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString, NSPrintInfo, NSPrintOperation, NSSharingService, NSSharingServiceNameComposeEmail, NSSharingServiceNameComposeMessage, NSSharingServiceNameSendViaAirDrop, NSSpeechSynthesizer, NSTextView, NSWindowTabbingMode};
    use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSSize, NSString, NSURL};
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSFontManager, NSFontTraitMask};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    thread_local! {
        static SPEAKER: RefCell<Option<Retained<NSSpeechSynthesizer>>> = const { RefCell::new(None) };
    }

    /// Mirrored from the speech calls on the main thread: the callers of
    /// `is_speaking` (menu builds, on the engine thread) cannot ask
    /// AppKit directly.
    static SPEAKING: AtomicBool = AtomicBool::new(false);

    /// Fonts picked in the font panel, until the engine thread's poll
    /// drains them (`take_font_picks`).
    static FONT_PICKS: std::sync::Mutex<Vec<(String, f64)>> = std::sync::Mutex::new(Vec::new());

    /// `NSFontManager.target` is not retained, so the `FontTarget` must
    /// outlive the panel. Kept as a leaked raw pointer: `Retained` is
    /// not `Sync` here and one shared instance is all the panel needs.
    static FONT_TARGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    objc2::define_class!(
        /// Receives `changeFont:` from the font panel (set as the font
        /// manager's target) and queues the pick for the engine thread.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "ScratchRunnerFontTarget"]
        struct FontTarget;

        unsafe impl NSObjectProtocol for FontTarget {}

        impl FontTarget {
            #[unsafe(method(changeFont:))]
            fn change_font(&self, sender: &NSFontManager) {
                if let Some(font) = sender.selectedFont() {
                    let family = font.familyName().map(|s| s.to_string()).unwrap_or_default();
                    FONT_PICKS.lock().expect("font picks").push((family, font.pointSize()));
                }
            }
        }
    );

    /// `NSApplication` — call only on the main thread (`on_main` runs
    /// there); `None` marks a miswired call.
    fn application() -> Option<Retained<NSApplication>> {
        MainThreadMarker::new().map(NSApplication::sharedApplication)
    }

    /// The app's document window. `mainWindow`/`keyWindow` are both nil
    /// while the app is inactive (windows resign main on deactivation),
    /// so fall back to the first main-capable window.
    fn main_window() -> Option<Retained<objc2_app_kit::NSWindow>> {
        application().and_then(|application| {
            application.mainWindow().or_else(|| application.keyWindow()).or_else(|| {
                application.windows().iter().find(|window| window.canBecomeMainWindow())
            })
        })
    }

    /// Runs `f` on the main thread. Menu `on_select` callbacks and editor
    /// events fire on NSH's engine thread, where AppKit calls are illegal
    /// (`MainThreadMarker` is unavailable there), so every AppKit body
    /// goes through here.
    fn on_main(f: impl FnOnce() + Send + 'static) {
        drop(nsh_core::element::run_on_main_thread(f));
    }

    pub fn emoji_and_symbols() {
        on_main(|| {
            if let Some(application) = application() {
                application.orderFrontCharacterPalette(None);
            }
        });
    }

    /// "Start Dictation…" — sends the standard action down the responder
    /// chain, where the focused text input client picks it up.
    pub fn start_dictation() {
        on_main(|| {
            if let Some(application) = application() {
                unsafe {
                    application.sendAction_to_from(sel!(startDictation:), None, None);
                }
            }
        });
    }

    /// Plain text + styled HTML on the general pasteboard.
    pub fn write_colored(html: &str, plain: &str) {
        let html = html.to_owned();
        let plain = plain.to_owned();
        on_main(move || {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // The extern pasteboard-type statics need `unsafe` to read.
            pasteboard.setString_forType(&NSString::from_str(&html), unsafe { NSPasteboardTypeHTML });
            pasteboard.setString_forType(&NSString::from_str(&plain), unsafe { NSPasteboardTypeString });
        });
    }

    /// Speaks `text`, starting over whatever is playing.
    pub fn speak(text: &str) {
        let text = text.to_owned();
        SPEAKING.store(true, Ordering::Relaxed);
        crate::workbench::state::bump_workspace_rev();
        on_main(move || {
            SPEAKER.with_borrow_mut(|slot| {
                let speaker = slot.get_or_insert_with(NSSpeechSynthesizer::new);
                speaker.stopSpeaking();
                speaker.startSpeakingString(&NSString::from_str(&text));
            });
        });
    }

    pub fn stop_speaking() {
        SPEAKING.store(false, Ordering::Relaxed);
        crate::workbench::state::bump_workspace_rev();
        on_main(|| {
            SPEAKER.with_borrow_mut(|slot| {
                if let Some(speaker) = slot.as_ref() {
                    speaker.stopSpeaking();
                }
            });
        });
    }

    pub fn is_speaking() -> bool {
        SPEAKING.load(Ordering::Relaxed)
    }

    /// File > Print… — the document's text through a throwaway NSTextView
    /// sized to the page (CodeRunner prints the source listing).
    pub fn print_document(title: &str, text: &str) {
        let title = title.to_owned();
        let text = text.to_owned();
        on_main(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let info = NSPrintInfo::sharedPrintInfo();
            let bounds = info.imageablePageBounds();
            let view = NSTextView::initWithFrame(mtm.alloc(), bounds);
            view.setString(&NSString::from_str(&text));
            if let Some(font) = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 11.0) {
                view.setFont(Some(&font));
            }
            view.setHorizontallyResizable(false);
            view.setVerticallyResizable(true);
            view.setMaxSize(NSSize::new(bounds.size.width, f64::MAX));
            view.setMinSize(bounds.size);
            if let Some(container) = unsafe { view.textContainer() } {
                container.setWidthTracksTextView(true);
                container.setHeightTracksTextView(false);
                container.setContainerSize(NSSize::new(bounds.size.width, f64::MAX));
            }
            view.sizeToFit();
            let operation = NSPrintOperation::printOperationWithView(&view);
            operation.setJobTitle(Some(&NSString::from_str(&title)));
            operation.setShowsPrintPanel(true);
            if let Some(window) = main_window() {
                unsafe {
                    operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
                        &window, None, None, std::ptr::null_mut(),
                    );
                }
            } else {
                operation.runOperation();
            }
        });
    }

    /// File > Page Setup… — the shared print info's layout panel.
    pub fn page_setup() {
        on_main(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                let layout = NSPageLayout::new(mtm);
                layout.runModal();
            }
        });
    }

    /// File > Share — the named sharing service on the document: its file
    /// URL when saved, its text otherwise.
    pub fn share(name: &'static str, path: Option<String>, text: String) {
        on_main(move || {
            let service_name = unsafe {
                match name {
                    "mail" => NSSharingServiceNameComposeEmail,
                    "message" => NSSharingServiceNameComposeMessage,
                    _ => NSSharingServiceNameSendViaAirDrop,
                }
            };
            let Some(service) = NSSharingService::sharingServiceNamed(service_name) else { return };
            let string = NSString::from_str(&text);
            let items = match path {
                Some(path) => {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
                    NSArray::from_slice(&[AsRef::<AnyObject>::as_ref(&*url)])
                }
                None => NSArray::from_slice(&[AsRef::<AnyObject>::as_ref(&*string)]),
            };
            unsafe { service.performWithItems(&items) };
        });
    }

    /// Text > Font > Show Fonts: the system's font panel with the
    /// document font, wired to `changeFont:` back through `FontTarget`.
    pub fn show_fonts(family: String, size: f64) {
        on_main(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let manager = NSFontManager::sharedFontManager(mtm);
            if FONT_TARGET.load(Ordering::Relaxed) == 0 {
                let target: Retained<FontTarget> = unsafe { objc2::msg_send_id![FontTarget::alloc(mtm), init] };
                FONT_TARGET.store(Retained::into_raw(target) as usize, Ordering::Relaxed);
            }
            let target = FONT_TARGET.load(Ordering::Relaxed) as *const AnyObject;
            unsafe {
                let current = NSFont::fontWithName_size(&NSString::from_str(&family), size)
                    .unwrap_or_else(|| NSFont::userFixedPitchFontOfSize(size).expect("Menlo exists"));
                manager.setSelectedFont_isMultiple(&current, false);
                manager.setTarget(Some(&*target));
                manager.setAction(sel!(changeFont:));
                manager.orderFrontFontPanel(None);
            }
        });
    }

    /// Picks queued by the font panel since the last drain.
    pub fn take_font_picks() -> Vec<(String, f64)> {
        std::mem::take(&mut *FONT_PICKS.lock().expect("font picks"))
    }

    /// Installed font families → whether the family resolves to a
    /// fixed-pitch font. `NSFontManager` is main-thread-only, so the list
    /// is queried once by `warm_font_cache` and cached for the engine
    /// thread's readers (the preferences font popup, the editor's family
    /// fallback).
    static FONT_FAMILIES: std::sync::Mutex<Option<BTreeMap<String, bool>>> = std::sync::Mutex::new(None);

    /// Fills `FONT_FAMILIES` once. Called at startup; `fixed_pitch_families`
    /// re-arms it when it runs first. The first fill bumps `FontRevision`
    /// so an open preferences pane repopulates its popup — the bump happens
    /// back on the engine thread, where signals live.
    pub fn warm_font_cache() {
        if FONT_FAMILIES.lock().expect("font families").is_some() {
            return;
        }
        nsh_core::prelude::spawn(async move {
            let _ = nsh_core::element::run_on_main_thread(|| {
                let Some(mtm) = MainThreadMarker::new() else { return };
                let manager = NSFontManager::sharedFontManager(mtm);
                let mut cache = BTreeMap::new();
                for name in manager.availableFontFamilies().iter() {
                    // `fontWithFamily:` resolves a member of the family
                    // (`fontWithName:` on a family name is not guaranteed).
                    let fixed = manager
                        .fontWithFamily_traits_weight_size(&name, NSFontTraitMask::empty(), 5, 12.0)
                        .is_some_and(|font| font.isFixedPitch());
                    cache.insert(name.to_string(), fixed);
                }
                FONT_FAMILIES.lock().expect("font families").replace(cache);
            }).await;
            crate::workbench::state::bump_font_rev();
        });
    }

    /// Installed families that resolve to a fixed-pitch font, sorted, for
    /// the preferences font popup. Before the warm completes the default
    /// monospaces are offered so the control is never blank.
    pub fn fixed_pitch_families() -> Vec<String> {
        let guard = FONT_FAMILIES.lock().expect("font families");
        match guard.as_ref() {
            Some(cache) => cache
                .iter()
                .filter(|(_, fixed)| **fixed)
                .map(|(family, _)| family.clone())
                .collect(),
            None => {
                drop(guard);
                warm_font_cache();
                vec!["Menlo".to_owned(), "SF Mono".to_owned()]
            }
        }
    }

    /// A persisted family that is not installed (a stale import, an
    /// uninstalled font) would render as the engine's fallback face: fall
    /// back to `default` (the configured monospace). Before the cache
    /// warms the name is trusted, as it usually resolves anyway.
    pub fn font_family_or_default(family: &str, default: &str) -> String {
        match FONT_FAMILIES.lock().expect("font families").as_ref() {
            Some(cache) => {
                if family.is_empty() || !cache.contains_key(family) {
                    default.to_owned()
                } else {
                    family.to_owned()
                }
            }
            None => {
                if family.is_empty() { default.to_owned() } else { family.to_owned() }
            }
        }
    }

    /// No NSWindow tab bar: our tabs are the workbench's own, so the
    /// system's Show Tab Bar / Show All Tabs items do not belong in View.
    pub fn disallow_window_tabbing() {
        on_main(|| {
            if let Some(window) = main_window() {
                window.setTabbingMode(NSWindowTabbingMode::Disallowed);
            }
        });
    }

    /// The main window's minimum size: below it the navigator/console
    /// minimums would leave the editor column and toolbar cramped past
    /// their usable widths. `AppOptions` has no minimum, so it is set
    /// on the window itself once it exists.
    pub fn set_min_window_size(width: f64, height: f64) {
        on_main(move || {
            if let Some(window) = main_window() {
                window.setMinSize(NSSize::new(width, height));
            }
        });
    }

    /// File > Close Window (the MenuRole would mark this app's File menu
    /// as the platform's windows menu — Window owns the roles).
    pub fn close_window() {
        on_main(|| {
            if let Some(window) = main_window() {
                window.performClose(None);
            }
        });
    }

    /// Window > Bring All to Front.
    pub fn bring_all_to_front() {
        on_main(|| {
            if let Some(application) = application() {
                application.activate();
                let windows: Vec<_> = application.windows().iter().collect();
                for window in windows.iter().rev() {
                    window.orderFront(None);
                }
            }
        });
    }

    /// Window > Merge All Windows — every window into the main window's
    /// tab group.
    pub fn merge_all_windows() {
        on_main(|| {
            if let Some(window) = main_window() {
                window.mergeAllWindows(None);
            }
        });
    }

    /// Window > Arrange in Front.
    pub fn arrange_in_front() {
        on_main(|| {
            if let Some(application) = application() {
                application.arrangeInFront(None);
            }
        });
    }

    /// Sends a standard edit-menu action down the responder chain.
    fn responder(sel: objc2::runtime::Sel) {
        on_main(move || {
            if let Some(application) = application() {
                unsafe {
                    application.sendAction_to_from(sel, None, None);
                }
            }
        });
    }

    /// Edit > Spelling and Grammar > Show Spelling and Grammar.
    pub fn show_spelling_panel() { responder(sel!(showGuessPanel:)); }
    /// Edit > Spelling and Grammar > Check Document Now.
    pub fn check_spelling_now() { responder(sel!(checkSpelling:)); }
    /// Edit > Spelling and Grammar > Check Spelling While Typing.
    pub fn toggle_spell_checking() { responder(sel!(toggleContinuousSpellChecking:)); }
    /// Edit > Spelling and Grammar > Check Grammar With Spelling.
    pub fn toggle_grammar_checking() { responder(sel!(toggleGrammarChecking:)); }
    /// Edit > Spelling and Grammar > Correct Spelling Automatically.
    pub fn toggle_auto_correction() { responder(sel!(toggleAutomaticSpellingCorrection:)); }
}

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn emoji_and_symbols() {}
    pub fn start_dictation() {}
    pub fn write_colored(_html: &str, _plain: &str) {}
    pub fn print_document(_title: &str, _text: &str) {}
    pub fn page_setup() {}
    pub fn share(_name: &'static str, _path: Option<String>, _text: String) {}
    pub fn show_fonts(_family: String, _size: f64) {}
    pub fn take_font_picks() -> Vec<(String, f64)> { Vec::new() }
    pub fn warm_font_cache() {}
    pub fn fixed_pitch_families() -> Vec<String> { vec!["Menlo".to_owned()] }
    pub fn font_family_or_default(family: &str, _default: &str) -> String { family.to_owned() }
    pub fn speak(_text: &str) {}
    pub fn stop_speaking() {}
    pub fn is_speaking() -> bool { false }
    pub fn disallow_window_tabbing() {}
    pub fn set_min_window_size(_width: f64, _height: f64) {}

    pub fn close_window() {}
    pub fn bring_all_to_front() {}
    pub fn merge_all_windows() {}
    pub fn arrange_in_front() {}
    pub fn show_spelling_panel() {}
    pub fn check_spelling_now() {}
    pub fn toggle_spell_checking() {}
    pub fn toggle_grammar_checking() {}
    pub fn toggle_auto_correction() {}
}

#[cfg(not(target_os = "macos"))]
pub use imp::*;
