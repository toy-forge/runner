//! macOS-native services the custom editor cannot inherit from the text
//! system: the character viewer (Edit > Emoji & Symbols), dictation, speech
//! (Edit > Speech), and rich-text pasteboard writes (Edit > Copy with
//! Colors). Everything degrades to a no-op off macOS.

/// Keystrokes captured on the macOS AppKit event monitor when a preferences
/// `KeyRecorder` capsule is armed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordedKey {
    /// Escape was pressed without modifiers: abort recording.
    Cancel,
    /// Delete/Backspace was pressed without modifiers: clear the shortcut.
    Clear,
    /// Canonical NSH stroke string (`ctrl+alt+shift+cmd+key`), accepted by
    /// `scratch_commands::Stroke::parse_nsh`.
    Stroke(String),
}

/// Computes the printable text bounds `(width, height)` inside `paper` minus `margins`.
pub(crate) fn printable_page_size(paper: (f64, f64), margins: (f64, f64, f64, f64)) -> (f64, f64) {
    (
        (paper.0 - margins.2 - margins.3).max(72.0),
        (paper.1 - margins.0 - margins.1).max(72.0),
    )
}

/// The pasteboard's HTML flavor, written next to the plain text so plain
/// targets still get the text.
#[cfg(target_os = "macos")]
#[allow(deprecated)] // NSSpeechSynthesizer, deprecated in macOS 14, still speaks.
mod imp {
    use super::RecordedKey;
    use block2::RcBlock;
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2::rc::Retained;
    use objc2::sel;
    use objc2_app_kit::{NSApplication, NSAttributedStringAppKitDocumentFormats, NSEvent, NSEventMask, NSEventModifierFlags, NSFont, NSPageLayout, NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString, NSPrintInfo, NSPrintOperation, NSSharingService, NSSharingServiceNameComposeEmail, NSSharingServiceNameComposeMessage, NSSharingServiceNameSendViaAirDrop, NSSpeechSynthesizer, NSTextView, NSWindowTabbingMode};
    use objc2_foundation::{NSArray, NSAttributedString, NSData, NSObject, NSObjectProtocol, NSSize, NSString, NSURL, NSUserDefaults, ns_string};
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSFontManager, NSFontTraitMask};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};

    thread_local! {
        static SPEAKER: RefCell<Option<Retained<NSSpeechSynthesizer>>> = const { RefCell::new(None) };
    }

    /// Mirrored from the speech calls on the main thread: the callers of
    /// `is_speaking` (menu builds, on the engine thread) cannot ask
    /// AppKit directly.
    static SPEAKING: AtomicBool = AtomicBool::new(false);

    /// True while a shortcut recorder capsule in Preferences is waiting for a key.
    static KEY_RECORDING_ARMED: AtomicBool = AtomicBool::new(false);
    static KEY_MONITOR_INSTALLED: AtomicBool = AtomicBool::new(false);
    static RECORDED_KEYS: std::sync::Mutex<Vec<RecordedKey>> = std::sync::Mutex::new(Vec::new());

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
                let base = sender
                    .selectedFont()
                    .or_else(|| NSFont::userFixedPitchFontOfSize(12.0));
                if let Some(base) = base {
                    let font = sender.convertFont(&base);
                    sender.setSelectedFont_isMultiple(&font, false);
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

    /// File > Print… — prints syntax-colored HTML source through a throwaway
    /// `NSTextView` sized to the printable page bounds with configured margins.
    pub fn print_colored_document(
        title: &str,
        html: &str,
        plain: &str,
        margins: (f64, f64, f64, f64),
    ) {
        let title = title.to_owned();
        let html = html.to_owned();
        let plain = plain.to_owned();
        on_main(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let info = NSPrintInfo::sharedPrintInfo();
            info.setTopMargin(margins.0);
            info.setBottomMargin(margins.1);
            info.setLeftMargin(margins.2);
            info.setRightMargin(margins.3);
            let paper = info.paperSize();
            let (print_w, print_h) = super::printable_page_size(
                (paper.width, paper.height),
                (info.topMargin(), info.bottomMargin(), info.leftMargin(), info.rightMargin()),
            );
            let bounds = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(0.0, 0.0),
                NSSize::new(print_w, print_h),
            );
            let view = NSTextView::initWithFrame(mtm.alloc(), bounds);
            let full_html = format!("<meta charset=\"utf-8\">{html}");
            let data = NSData::with_bytes(full_html.as_bytes());
            let loaded_html = unsafe {
                NSAttributedString::initWithHTML_documentAttributes(
                    NSAttributedString::alloc(),
                    &data,
                    None,
                )
            };
            if let Some(attributed) = loaded_html
                && let Some(storage) = unsafe { view.textStorage() }
            {
                storage.setAttributedString(&attributed);
            } else {
                view.setString(&NSString::from_str(&plain));
                if let Some(font) = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 9.0) {
                    view.setFont(Some(&font));
                }
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

    /// Spawns a new ScratchRunner window process, optionally opening `paths`.
    pub fn spawn_window(paths: &[std::path::PathBuf]) {
        let Ok(exe) = std::env::current_exe() else { return; };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--new-window");
        for path in paths {
            cmd.arg("--open").arg(path);
        }
        let _ = cmd.spawn();
    }

    /// Detaches an open document (even if untitled or modified) into a fresh window.
    pub fn spawn_detached_doc(id: scratch_document::DocumentId) {
        let text = crate::workbench::state::text(id);
        let saved = crate::workbench::state::with(|workspace| {
            workspace.doc(id).map(|doc| crate::workbench::persistence::SavedDocument {
                document: doc.document.clone(),
                title: doc.title.clone(),
                text: text.unwrap_or_else(|| doc.text.clone()),
                diagnostics: (!doc.diagnostics_data.is_empty()).then(|| doc.diagnostics_data.clone()),
            })
        });
        let Some(saved) = saved else { return; };
        let Ok(bytes) = serde_json::to_vec(&saved) else { return; };
        let handoff = std::env::temp_dir().join(format!("scratchrunner-handoff-{}.json", uuid::Uuid::new_v4()));
        {
            use std::io::Write;
            #[cfg(unix)]
            let file_res = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&handoff)
            };
            #[cfg(not(unix))]
            let file_res = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&handoff);
            let Ok(mut f) = file_res else { return; };
            if f.write_all(&bytes).is_err() {
                let _ = std::fs::remove_file(&handoff);
                return;
            }
        }
        let Ok(exe) = std::env::current_exe() else { return; };
        if std::process::Command::new(exe)
            .arg("--new-window")
            .arg("--handoff")
            .arg(&handoff)
            .spawn()
            .is_ok()
        {
            crate::workbench::state::remove(id);
        } else {
            let _ = std::fs::remove_file(&handoff);
        }
    }

    /// Enable the WebKit Web Inspector ("Inspect Element") context menu in
    /// embedded `WKWebView` panes (Console Web/Markdown preview & Docs).
    pub fn enable_webview_developer_extras() {
        NSUserDefaults::standardUserDefaults()
            .setBool_forKey(true, ns_string!("WebKitDeveloperExtras"));
    }

    /// Virtual key codes for `F1` through `F20` (same order as `nsh-platform-macos`).
    const FUNCTION_KEYS: [u16; 20] = [
        122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
    ];
    const US_KEYS: &[u8; 51] = b"asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0\0`";

    fn nsh_key_for_event(event: &NSEvent) -> Option<(String, bool)> {
        let code = event.keyCode();
        // Bare modifier presses (`Command`, `Shift`, `CapsLock`, `Option`, `Control`, `Fn`).
        if matches!(code, 54..=63) {
            return None;
        }
        let mut is_named = true;
        let name = match code {
            53 => "escape".to_owned(),
            36 | 76 => "enter".to_owned(),
            48 => "tab".to_owned(),
            49 => {
                is_named = false;
                "space".to_owned()
            }
            51 => "backspace".to_owned(),
            117 => "delete".to_owned(),
            114 => "insert".to_owned(),
            123 => "left".to_owned(),
            124 => "right".to_owned(),
            125 => "down".to_owned(),
            126 => "up".to_owned(),
            115 => "home".to_owned(),
            119 => "end".to_owned(),
            116 => "pageup".to_owned(),
            121 => "pagedown".to_owned(),
            _ => {
                if let Some(idx) = FUNCTION_KEYS.iter().position(|&k| k == code) {
                    format!("f{}", idx + 1)
                } else {
                    is_named = false;
                    let ch = US_KEYS
                        .get(usize::from(code))
                        .copied()
                        .filter(|&c| c != 0)
                        .map(|c| c as char)
                        .or_else(|| {
                            event
                                .charactersIgnoringModifiers()
                                .and_then(|s| s.to_string().chars().next())
                                .filter(|c| c.is_ascii_graphic())
                                .map(|c| c.to_ascii_lowercase())
                        })?;
                    if ch == '+' {
                        "plus".to_owned()
                    } else {
                        ch.to_string()
                    }
                }
            }
        };
        Some((name, is_named))
    }

    fn handle_recorded_keydown(event: &NSEvent) -> bool {
        if !KEY_RECORDING_ARMED.load(Ordering::Relaxed) {
            return false;
        }
        let code = event.keyCode();
        if matches!(code, 54..=63) {
            return true;
        }
        let flags = event.modifierFlags();
        let ctrl = flags.contains(NSEventModifierFlags::Control);
        let alt = flags.contains(NSEventModifierFlags::Option);
        let shift = flags.contains(NSEventModifierFlags::Shift);
        let cmd = flags.contains(NSEventModifierFlags::Command);
        if code == 53 && !ctrl && !alt && !cmd {
            KEY_RECORDING_ARMED.store(false, Ordering::Relaxed);
            RECORDED_KEYS.lock().expect("recorded keys").push(RecordedKey::Cancel);
            return true;
        }
        if (code == 51 || code == 117) && !ctrl && !alt && !shift && !cmd {
            KEY_RECORDING_ARMED.store(false, Ordering::Relaxed);
            RECORDED_KEYS.lock().expect("recorded keys").push(RecordedKey::Clear);
            return true;
        }
        let Some((key_name, is_named)) = nsh_key_for_event(event) else {
            return true;
        };
        // Require a non-shift modifier for plain typing keys so normal text
        // input keys aren't accidentally bound unless modified or F/nav keys.
        let is_function = key_name.starts_with('f') && key_name.len() > 1;
        if !(ctrl || alt || cmd || is_function || (is_named && !matches!(key_name.as_str(), "tab" | "enter"))) {
            KEY_RECORDING_ARMED.store(false, Ordering::Relaxed);
            RECORDED_KEYS.lock().expect("recorded keys").push(RecordedKey::Cancel);
            return true;
        }
        let mut parts = Vec::with_capacity(5);
        if ctrl { parts.push("ctrl"); }
        if alt { parts.push("alt"); }
        if shift { parts.push("shift"); }
        if cmd { parts.push("cmd"); }
        parts.push(&key_name);
        let stroke = parts.join("+");
        KEY_RECORDING_ARMED.store(false, Ordering::Relaxed);
        RECORDED_KEYS.lock().expect("recorded keys").push(RecordedKey::Stroke(stroke));
        true
    }

    fn ensure_key_monitor() {
        if KEY_MONITOR_INSTALLED.swap(true, Ordering::AcqRel) {
            return;
        }
        on_main(|| {
            let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
                let ns_event = unsafe { event.as_ref() };
                if handle_recorded_keydown(ns_event) {
                    std::ptr::null_mut()
                } else {
                    event.as_ptr()
                }
            });
            if let Some(monitor) = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
            } {
                // Keep the local monitor alive for the lifetime of the app.
                std::mem::forget(monitor);
            } else {
                KEY_MONITOR_INSTALLED.store(false, Ordering::Release);
            }
        });
    }

    /// Arms or disarms the AppKit local `KeyDown` monitor for the interactive
    /// `KeyRecorder` widget in Preferences.
    pub fn set_key_recording(armed: bool) {
        if armed {
            RECORDED_KEYS.lock().expect("recorded keys").clear();
            ensure_key_monitor();
        }
        KEY_RECORDING_ARMED.store(armed, Ordering::Relaxed);
    }

    /// Drains the next key captured by [`set_key_recording`], if any.
    pub fn take_recorded_key() -> Option<RecordedKey> {
        let mut guard = RECORDED_KEYS.lock().expect("recorded keys");
        if guard.is_empty() { None } else { Some(guard.remove(0)) }
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::RecordedKey;

    pub fn emoji_and_symbols() {}
    pub fn start_dictation() {}
    pub fn write_colored(_html: &str, _plain: &str) {}
    pub fn print_colored_document(
        _title: &str,
        _html: &str,
        _plain: &str,
        _margins: (f64, f64, f64, f64),
    ) {}
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
    pub fn spawn_window(_paths: &[std::path::PathBuf]) {}
    pub fn spawn_detached_doc(_id: scratch_document::DocumentId) {}
    pub fn enable_webview_developer_extras() {}
    pub fn set_key_recording(_armed: bool) {}
    pub fn take_recorded_key() -> Option<RecordedKey> { None }
}

#[cfg(not(target_os = "macos"))]
pub use imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_page_size_subtracts_margins() {
        assert_eq!(
            printable_page_size((612.0, 792.0), (40.0, 40.0, 40.0, 40.0)),
            (532.0, 712.0)
        );
        assert_eq!(
            printable_page_size((100.0, 100.0), (40.0, 40.0, 40.0, 40.0)),
            (72.0, 72.0)
        );
    }
}
