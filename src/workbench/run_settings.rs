//! Run Settings (Shift-Cmd-R): the run command, compile flags, arguments,
//! program input, environment and working directory of the selected
//! document. The sheet edits a draft in its own signals; Done writes it,
//! Cancel and Escape drop it. `mod.rs` mounts `overlay()` once at the root.
//!
//! Everything the sheet edits is stored on `Document.run`, so the recovery
//! journal keeps it; [`runner_settings`] turns it into what a run uses.
//!
//! Motion inventory (the kit's `motion` tokens and recipes):
//!
//! | Change | Motion | Reduced motion |
//! |---|---|---|
//! | Sheet opens | scrim fades in; the card drops `DISTANCE_SHEET` 12pt, fades and scales 0.97 to 1, `slow`, ease-out | both land at once |
//! | Sheet closes | removed with the dialog layer (NSH has no leave motion for a dialog) | the same |
//! | Button, icon button, segment hover and press | the kit's wash (`instant`) and press scale (`fast`, soft settle) | no scale |
//! | Working directory segment chosen | the kit's sliding pill, `base` | instant |
//! | Done enabled or disabled | the kit's disabled look (45%), swapped on the change | the same |
//! | "specific to this file" banner shows or hides | `motion::animate_panel` (`base`, ease-in-out); the banner fades in rising 4pt, `base` | instant |
//! | Environment row added or removed | `animate_panel`; a new row fades in rising 4pt | instant |
//! | Custom folder row shows or hides | `animate_panel` | instant |
//! | A field's problem appears | fades in rising 4pt, `base` | instant |
//! | Field focus | the kit's focus ring | the same |
//! | Drags, loading, live content | none on this sheet | |

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use nsh_base::dialog;
use nsh_core::file_dialog::OpenDialog;
use nsh_core::keymap::When;
use scratch_document::{Document, DocumentId, RunFolder, RunSettings as SavedRun};
use scratch_runner::{ProgramInput, RunSettings as RunnerSettings, WorkingDirectory};
use scratch_ui_kit::prelude::*;
use scratch_ui_kit::tokens;

use super::state::{self, with};

// ------------------------------------------------------------------ model

/// The runner's settings for a document's run settings.
/// `execution::request_for` builds its request with this.
pub fn runner_settings(document: &Document) -> RunnerSettings { settings_for(&document.run) }

fn settings_for(run: &SavedRun) -> RunnerSettings {
    RunnerSettings {
        run_command: run.run_command.clone().filter(|command| !command.trim().is_empty()),
        compile_flags: run.compiler_flags.clone(),
        arguments: run.arguments.clone(),
        input: (!run.input.is_empty()).then(|| ProgramInput::Text(run.input.clone())),
        input_via_environment: run.input_via_environment,
        environment: run.environment.clone(),
        working_directory: match &run.working_folder {
            RunFolder::File => WorkingDirectory::FileDirectory,
            RunFolder::Home => WorkingDirectory::Default,
            RunFolder::Custom(path) => WorkingDirectory::Custom(path.clone()),
        },
        ..RunnerSettings::default()
    }
}

/// The language's side of the sheet.
#[derive(Debug, Clone, PartialEq, Default)]
struct Defaults {
    command: String,
    flags: String,
    compiles: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Folder {
    #[default]
    File,
    Home,
    Custom,
}

impl Folder {
    const ALL: [Folder; 3] = [Folder::File, Folder::Home, Folder::Custom];
    fn title(self) -> &'static str { match self { Folder::File => "File's Folder", Folder::Home => "Home Folder", Folder::Custom => "Other Folder" } }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvRow {
    id: u64,
    name: String,
    value: String,
}

/// The sheet's values, as typed.
#[derive(Debug, Clone, PartialEq, Default)]
struct Draft {
    run_command: String,
    compile_flags: String,
    arguments: String,
    input: String,
    input_via_environment: bool,
    environment: Vec<EnvRow>,
    folder: Folder,
    custom_folder: String,
}

impl Draft {
    /// The sheet as it opens: an override, or the language's default to edit.
    fn of(run: &SavedRun, defaults: &Defaults) -> Self {
        let (folder, custom_folder) = match &run.working_folder {
            RunFolder::File => (Folder::File, String::new()),
            RunFolder::Home => (Folder::Home, String::new()),
            RunFolder::Custom(path) => (Folder::Custom, path.display().to_string()),
        };
        Draft {
            run_command: run.run_command.clone().unwrap_or_else(|| defaults.command.clone()),
            compile_flags: run.compiler_flags.clone().unwrap_or_else(|| defaults.flags.clone()),
            arguments: run.arguments.clone(),
            input: run.input.clone(),
            input_via_environment: run.input_via_environment,
            environment: run.environment.iter().zip(0..).map(|((name, value), id)| EnvRow { id, name: name.clone(), value: value.clone() }).collect(),
            folder,
            custom_folder,
        }
    }
}

/// Where a problem shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Arguments,
    Environment,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Problem {
    field: Field,
    message: String,
}

impl Problem {
    fn new(field: Field, message: impl Into<String>) -> Self { Problem { field, message: message.into() } }
}

/// A run command that is empty or the language's own is no override.
fn command_override(text: &str, default: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text != default.trim()).then(|| text.to_owned())
}

/// Compile flags equal to the language's are no override; empty flags are
/// one when the language has some (compile without them).
fn flags_override(text: &str, default: &str) -> Option<String> {
    let text = text.trim();
    (text != default.trim()).then(|| text.to_owned())
}

/// A name a shell can export.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The environment rows as variables; blank rows are ignored.
fn environment(rows: &[EnvRow]) -> Result<BTreeMap<String, String>, String> {
    let mut variables = BTreeMap::new();
    for row in rows {
        let name = row.name.trim();
        if name.is_empty() && row.value.is_empty() { continue; }
        if name.is_empty() { return Err(format!("The value “{}” needs a variable name.", row.value)); }
        if !valid_name(name) { return Err(format!("“{name}” is not a variable name. Use letters, digits and _, not starting with a digit.")); }
        if variables.insert(name.to_owned(), row.value.clone()).is_some() { return Err(format!("“{name}” is set twice.")); }
    }
    Ok(variables)
}

/// A folder typed by the user: absolute, or under `~`.
fn custom_folder(text: &str, home: Option<PathBuf>) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() { return Err("Choose a folder, or run in the file's folder.".into()); }
    let path = match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(text),
    };
    if path.is_absolute() { Ok(path) } else { Err("Enter a full path, starting with / or ~.".into()) }
}

fn home() -> Option<PathBuf> { std::env::var_os("HOME").map(PathBuf::from) }

/// The draft as the document's settings, or what stops it.
fn settle(draft: &Draft, defaults: &Defaults, home: Option<PathBuf>) -> Result<SavedRun, Vec<Problem>> {
    let mut problems = Vec::new();
    let arguments = RunnerSettings { arguments: draft.arguments.clone(), ..RunnerSettings::default() };
    if arguments.argument_list().is_err() { problems.push(Problem::new(Field::Arguments, "A quote is not closed.")); }
    let environment = environment(&draft.environment).unwrap_or_else(|message| { problems.push(Problem::new(Field::Environment, message)); BTreeMap::new() });
    let working_folder = match draft.folder {
        Folder::File => RunFolder::File,
        Folder::Home => RunFolder::Home,
        Folder::Custom => match custom_folder(&draft.custom_folder, home) {
            Ok(path) => RunFolder::Custom(path),
            Err(message) => { problems.push(Problem::new(Field::Folder, message)); RunFolder::File }
        },
    };
    if !problems.is_empty() { return Err(problems); }
    Ok(SavedRun {
        run_command: command_override(&draft.run_command, &defaults.command),
        compiler_flags: if defaults.compiles { flags_override(&draft.compile_flags, &defaults.flags) } else { None },
        arguments: draft.arguments.trim().to_owned(),
        input: draft.input.clone(),
        environment,
        input_via_environment: draft.input_via_environment,
        working_folder,
    })
}

/// The argument list as the program gets it: `[one] [two words]`.
fn argument_preview(arguments: &str) -> Option<String> {
    let settings = RunnerSettings { arguments: arguments.to_owned(), ..RunnerSettings::default() };
    let list = settings.argument_list().ok()?;
    (!list.is_empty()).then(|| list.iter().map(|argument| format!("[{argument}]")).collect::<Vec<_>>().join(" "))
}

// ------------------------------------------------------------------ sheet

/// The document the sheet is open for.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    id: DocumentId,
    title: String,
    language_id: String,
    language: String,
    folder: Option<PathBuf>,
    defaults: Defaults,
    draft: Draft,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Open(Option<Target>);

fn open_sheet() -> Signal<Open> { global::<Open>() }

/// Opens the sheet for the selected document.
pub fn open() {
    let Some(id) = with(|workspace| workspace.active) else { state::notice("Open a document to edit its run settings."); return; };
    let target = with(|workspace| {
        let doc = workspace.doc(id)?;
        let language = workspace.registry.as_ref().and_then(|registry| registry.get(&doc.document.language));
        let defaults = language.map(|def| Defaults { command: def.run.command.clone(), flags: def.run.flags.clone(), compiles: def.has_compile_step() }).unwrap_or_default();
        Some(Target {
            id,
            title: doc.title.clone(),
            language_id: doc.document.language.clone(),
            language: language.map_or_else(|| doc.document.language.clone(), |def| def.name.clone()),
            folder: state::saved_path(&doc.document).and_then(|path| path.parent().map(PathBuf::from)),
            draft: Draft::of(&doc.document.run, &defaults),
            defaults,
        })
    });
    if let Some(target) = target { open_sheet().set(Open(Some(target))); }
}

fn show_help() {
    Owner::detached().run(|| spawn(async {
        let _ = nsh_base::alert_dialog()
            .title("Run Settings")
            .description(
                "Run Command overrides the language's launch command ($filename is replaced with the source file, $compiler with the compiled output).\n\n\
                 Compile Flags override compiler arguments for compiled languages.\n\n\
                 Program Input is piped to standard input, or provided as a file path in $CR_INPUT when checked."
            )
            .action("ok", "OK")
            .show()
            .await;
    }));
}

fn make_default(language_id: &str, language_name: &str, compiles: bool, run_command: &str, compile_flags: &str) -> bool {
    let command = run_command.trim().to_owned();
    let flags = compile_flags.trim().to_owned();
    let result = with(|workspace| {
        let Some(registry) = workspace.registry.as_mut() else {
            return Err("Language registry not loaded.".to_owned());
        };
        let Some(mut def) = registry.get(language_id).cloned() else {
            return Err("Unknown language.".to_owned());
        };
        if !command.is_empty() {
            def.run.command = command.clone();
        }
        if compiles {
            def.run.flags = flags.clone();
        }
        registry.save(def).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => {
            global::<state::RegistryRevision>().update(|rev| rev.0 += 1);
            state::notice(format!("Saved default run settings for {language_name}."));
            true
        }
        Err(error) => {
            state::notice(format!("Could not save default settings: {error}"));
            false
        }
    }
}

fn close() { open_sheet().set(Open(None)); }

/// Writes the settled draft to the document and closes the sheet.
fn commit(id: DocumentId, run: SavedRun) {
    let changed = with(|workspace| {
        let Some(doc) = workspace.doc_mut(id) else { return false; };
        if doc.document.run == run { return false; }
        doc.document.run = run;
        workspace.changed();
        true
    });
    close();
    if changed { state::publish(); }
}

/// The sheet, mounted once at the root.
pub fn overlay() -> impl IntoElement {
    let shown = computed(move || open_sheet().with(|open| open.0.as_ref().map(|target| target.id)));
    view().child(dynamic(move || {
        let shown = shown.get();
        untracked(move || match (shown, open_sheet().get().0) {
            (Some(_), Some(target)) => sheet(target).into_element(),
            _ => view().into_element(),
        })
    }))
}

/// Fades in rising 4pt: a banner, a row or a message arriving.
fn rise() -> MotionSpec { motion::fade_in_rise(motion::base()) }

fn sheet(target: Target) -> impl IntoElement {
    let Target { id, title, language_id, language, folder, defaults, draft } = target;
    let run_command = signal(draft.run_command.clone());
    let compile_flags = signal(draft.compile_flags.clone());
    let arguments = signal(draft.arguments.clone());
    let program_input = signal(draft.input.clone());
    let via_environment = signal(draft.input_via_environment);
    let rows = signal(draft.environment.clone());
    let next_row = stored(draft.environment.len() as u64);
    let chosen_folder = signal(draft.folder);
    let custom = signal(draft.custom_folder.clone());
    let compiles = defaults.compiles;
    let initial_cmd = defaults.command.clone();
    let defaults = signal(defaults);

    let current = move || Draft {
        run_command: run_command.get(),
        compile_flags: compile_flags.get(),
        arguments: arguments.get(),
        input: program_input.get(),
        input_via_environment: via_environment.get(),
        environment: rows.get(),
        folder: chosen_folder.get(),
        custom_folder: custom.get(),
    };
    let outcome = computed(move || defaults.with(|defaults| settle(&current(), defaults, home())));
    // The Done button is rebuilt only when this flips, not on every keystroke.
    let ready = computed(move || outcome.with(Result::is_ok));
    let problem = move |field: Field| outcome.with(|outcome| outcome.as_ref().err().and_then(|problems| problems.iter().find(|p| p.field == field).map(|p| p.message.clone())));
    let done = move || {
        if let Ok(run) = outcome.get_untracked() { commit(id, run); }
    };
    on_key_with("mod+enter", KeyOptions { when: When::Always, ..KeyOptions::default() }, move |_| done());

    let enter = |from: f32| MotionSpec::new(KeyframeTiming::new(motion::slow()).easing(motion::EASE_ENTER)).values(MotionChannel::Opacity, [from, 1.0]);
    let scrim_motion = motion().entering(enter(0.0));
    let sheet_motion = motion().entering(enter(0.0)
        .values(MotionChannel::TranslateY, [-motion::DISTANCE_SHEET, 0.0])
        .values(MotionChannel::Scale, [motion::POPUP_SCALE, 1.0]));

    let body = view().px(px(20.0)).pb(px(4.0)).gap(px(12.0))
        .child(labelled("Run Command:", view().gap(px(6.0))
            .child(code_field(move || Some(SharedString::from(run_command.get())), {
                move |text: &SharedString| {
                    let default = defaults.with_untracked(|d| d.command.clone());
                    set_with_banner(run_command, text.to_string(), &default, command_override);
                }
            }, CodeFieldOptions::new("shell").min_rows(2).max_rows(4).accessibility_label("Run Command")
                .placeholder(if initial_cmd.trim().is_empty() { "This language has no run command".to_owned() } else { initial_cmd })))
            .child(banner("Run command is specific to this file.", {
                move || {
                    let default = defaults.with(|d| d.command.clone());
                    command_override(&run_command.get(), &default).is_some()
                }
            }, {
                move || {
                    let default = defaults.with_untracked(|d| d.command.clone());
                    set_with_banner(run_command, default.clone(), &default, command_override);
                }
            }))))
        .child(labelled("Arguments:", view().gap(px(4.0))
            .child(text_field("None", move || SharedString::from(arguments.get()), move |text: &str| arguments.set(text.to_owned()),
                TextFieldOptions::new().mono(true).accessibility_label("Arguments"), None))
            .child(hint_or_problem(move || argument_preview(&arguments.get()).map(|list| format!("The program gets {list}.")), move || problem(Field::Arguments)))))
        .when(compiles, |body| body.child(labelled("Compile Flags:", view().gap(px(6.0))
            .child(text_field("None", move || SharedString::from(compile_flags.get()), {
                move |text: &str| {
                    let default = defaults.with_untracked(|d| d.flags.clone());
                    set_with_banner(compile_flags, text.to_owned(), &default, flags_override);
                }
            }, TextFieldOptions::new().mono(true).accessibility_label("Compile Flags"), None))
            .child(banner("Compile flags are specific to this file.", {
                move || {
                    let default = defaults.with(|d| d.flags.clone());
                    flags_override(&compile_flags.get(), &default).is_some()
                }
            }, {
                move || {
                    let default = defaults.with_untracked(|d| d.flags.clone());
                    set_with_banner(compile_flags, default.clone(), &default, flags_override);
                }
            })))))
        .child(labelled("Program Input:", view().gap(px(8.0))
            .child(text_area("Text the program reads", move || SharedString::from(program_input.get()), move |text: &str| program_input.set(text.to_owned()),
                TextFieldOptions::new().mono(true).accessibility_label("Program Input"), None))
            .child(checkbox("Pass it as the file named by $CR_INPUT instead of standard input", move || via_environment.get(),
                CheckboxOptions::default(), move |next: bool| via_environment.set(next)))))
        .child(labelled("Environment:", environment_rows(rows, next_row, move || problem(Field::Environment))))
        .child(labelled("Working Directory:", folder_choice(chosen_folder, custom, folder, move || problem(Field::Folder))));

    dialog()
        .animation(ModalAnimation::None)
        .label("Run Settings")
        .on_cancel(close)
        .flex_1()
        // A press outside does nothing: the draft is only dropped by Cancel or Escape.
        .child(view().absolute().inset_0().bg(tokens::scrim).bind_motion(&scrim_motion))
        .child(view().flex_1().items_center().pt(px(44.0)).pb(px(12.0))
            .child(view().bind_motion(&sheet_motion)
                .w(px(500.0)).min_h(px(0.0)).flex_shrink(1.0)
                .rounded(px(tokens::radius_xl())).border_1().border_color(tokens::border)
                .shadow_lg().shadow_color(tokens::shadow())
                .bg(tokens::background)
                .child(view().px(px(20.0)).pt(px(20.0)).pb(px(12.0)).gap(px(2.0))
                    .child(type_subhead().semibold().apply(text("Run Settings")).text_color(tokens::foreground))
                    .child(type_body().apply(text(format!("{title} · {language}"))).number_of_lines(1).text_color(tokens::muted_foreground)))
                .child(scroll().flex_shrink(1.0).min_h(px(0.0)).child(body))
                .child(view().flex_row().items_center().gap(px(8.0)).px(px(20.0)).pt(px(16.0)).pb(px(20.0))
                    .child(icon_button(IconName::HelpCircle, "Run Settings Help", IconButtonOptions::new().size(ControlSize::Sm), show_help))
                    .child(button("Make Default", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Lg), {
                        let language_id = language_id.clone();
                        let language = language.clone();
                        move || {
                            let cmd = run_command.get_untracked();
                            let flg = compile_flags.get_untracked();
                            if make_default(&language_id, &language, compiles, &cmd, &flg) {
                                defaults.update(|d| {
                                    let c = cmd.trim();
                                    if !c.is_empty() {
                                        d.command = c.to_owned();
                                    }
                                    if compiles {
                                        d.flags = flg.trim().to_owned();
                                    }
                                });
                            }
                        }
                    }))
                    .child(view().flex_1())
                    .child(button("Cancel", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Lg), close))
                    .child(dynamic(move || {
                        let ready = ready.get();
                        button("Done", ButtonOptions::new().variant(ButtonVariant::Primary).size(ControlSize::Lg).disabled(!ready), done)
                    })))))
}

// ------------------------------------------------------------------ parts

/// Sets a field, animating the layout when its "specific to this file"
/// banner shows or hides.
fn set_with_banner(value: Signal<String>, next: String, default: &str, specific: fn(&str, &str) -> Option<String>) {
    let was = value.with_untracked(|text| specific(text, default).is_some());
    if was != specific(&next, default).is_some() { motion::animate_panel(); }
    value.set(next);
}

/// A field under its label (DESIGN.md 8.10 stacks them, 4 apart).
fn labelled(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    view().gap(px(4.0))
        .child(type_body().medium().apply(text(label)).text_color(tokens::foreground))
        .child(control)
}

fn caption(message: impl IntoReactive<SharedString> + 'static) -> impl IntoElement {
    type_caption().apply(text(message)).text_color(tokens::muted_foreground)
}

/// A hint under a field, replaced by the field's problem while it has one.
fn hint_or_problem(hint: impl Fn() -> Option<String> + 'static, problem: impl Fn() -> Option<String> + Copy + 'static) -> impl IntoElement {
    let hint = Rc::new(hint);
    view()
        .child(when(move || problem().is_none(), move || {
            let hint = hint.clone();
            caption(move || SharedString::from(hint().unwrap_or_default()))
        }))
        .child(when(move || problem().is_some(), move || {
            let shown = motion().entering(rise());
            view().bind_motion(&shown).child(type_caption().apply(text(move || problem().unwrap_or_default())).text_color(tokens::destructive))
        }))
}

/// "Run command is specific to this file." with its Revert.
fn banner(message: &'static str, shown: impl Fn() -> bool + 'static, revert: impl Fn() + 'static) -> impl IntoElement {
    let revert = Rc::new(revert);
    when(shown, move || {
        let revert = revert.clone();
        let appear = motion().entering(rise());
        view().bind_motion(&appear).flex_row().items_center().gap(px(8.0)).pl(px(10.0)).pr(px(4.0)).py(px(4.0))
            .rounded(px(tokens::radius())).bg(tokens::muted)
            .child(type_caption().apply(text(message)).flex_1().text_color(tokens::muted_foreground))
            .child(button("Revert to Default", ButtonOptions::new().variant(ButtonVariant::Ghost).size(ControlSize::Sm), move || revert()))
    })
}

fn environment_rows(rows: Signal<Vec<EnvRow>>, next_row: Stored<u64>, problem: impl Fn() -> Option<String> + Copy + 'static) -> impl IntoElement {
    let first_new_row = next_row.with(|next| *next);
    let add = move || {
        let id = next_row.with(|next| { let id = *next; *next += 1; id });
        motion::animate_panel();
        rows.update(|rows| rows.push(EnvRow { id, name: String::new(), value: String::new() }));
    };
    view().gap(px(6.0))
        .child(each(move || rows.get(), |row: &EnvRow| row.id, move |row, _| {
            let id = row.get_untracked().id;
            let edit = move |change: &dyn Fn(&mut EnvRow)| rows.update(|rows| { if let Some(row) = rows.iter_mut().find(|row| row.id == id) { change(row); } });
            let appear = if id >= first_new_row { motion().entering(rise()) } else { motion() };
            view().bind_motion(&appear).flex_row().items_center().gap(px(6.0))
                .child(view().w(px(150.0)).child(text_field("NAME", move || SharedString::from(row.with(|row| row.name.clone())),
                    move |text: &str| edit(&|row| row.name = text.to_owned()), TextFieldOptions::new().mono(true).accessibility_label("Variable name"), None)))
                .child(type_body().apply(text("=")).text_color(tokens::muted_foreground))
                .child(view().flex_1().min_w(px(0.0)).child(text_field("value", move || SharedString::from(row.with(|row| row.value.clone())),
                    move |text: &str| edit(&|row| row.value = text.to_owned()), TextFieldOptions::new().mono(true).accessibility_label("Variable value"), None)))
                .child(icon_button(IconName::Minus, "Remove Variable", IconButtonOptions::new().size(ControlSize::Sm),
                    move || { motion::animate_panel(); rows.update(|rows| rows.retain(|row| row.id != id)); }))
        }))
        .child(when(move || rows.with(Vec::is_empty), || caption("None of its own. The Shell preferences' variables still apply.")))
        .child(view().flex_row().child(button("Add Variable", ButtonOptions::new().variant(ButtonVariant::Default).size(ControlSize::Sm).icon(IconName::Plus), add)))
        .child(hint_or_problem(|| None, problem))
}

fn folder_choice(chosen: Signal<Folder>, custom: Signal<String>, file_folder: Option<PathBuf>, problem: impl Fn() -> Option<String> + Copy + 'static) -> impl IntoElement {
    let choose = move || {
        spawn(async move {
            match pick_paths(OpenDialog::folders().button_label("Choose")).await {
                Ok(Some(paths)) => if let Some(path) = paths.into_iter().next() { custom.set(path.display().to_string()); },
                Ok(None) => {},
                Err(error) => state::notice(format!("Could not open the folder dialog: {error}")),
            }
        });
    };
    let hint = move || match chosen.get() {
        Folder::File => Some(file_folder.as_ref().map_or_else(|| "Unsaved documents run in a temporary folder.".to_owned(), |folder| format!("Runs in {}.", folder.display()))),
        Folder::Home => Some("Runs in your home folder.".to_owned()),
        Folder::Custom => None,
    };
    view().gap(px(6.0))
        .child(view().flex_row().child(segmented("Working Directory", Folder::ALL.map(|folder| Segment::new(folder.title())),
            move || Folder::ALL.iter().position(|folder| *folder == chosen.get()).unwrap_or(0),
            move |index: usize| {
                let folder = Folder::ALL[index.min(Folder::ALL.len() - 1)];
                if (folder == Folder::Custom) != (chosen.get_untracked() == Folder::Custom) { motion::animate_panel(); }
                chosen.set(folder);
            },
            SegmentedOptions::default())))
        .child(when(move || chosen.get() == Folder::Custom, move || {
            view().flex_row().items_center().gap(px(6.0))
                .child(view().flex_1().min_w(px(0.0)).child(text_field("/path/to/folder", move || SharedString::from(custom.get()), move |text: &str| custom.set(text.to_owned()),
                    TextFieldOptions::new().mono(true).accessibility_label("Folder"), None)))
                .child(button("Choose…", ButtonOptions::new().variant(ButtonVariant::Default).icon(IconName::Folder), choose))
        }))
        .child(hint_or_problem(hint, problem))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python() -> Defaults { Defaults { command: "python3 -u $filename".into(), flags: String::new(), compiles: false } }
    fn c() -> Defaults { Defaults { command: "$compiler".into(), flags: "-std=c11".into(), compiles: true } }
    fn row(id: u64, name: &str, value: &str) -> EnvRow { EnvRow { id, name: name.into(), value: value.into() } }

    #[test]
    fn an_empty_or_default_run_command_is_the_languages() {
        assert_eq!(command_override("", "python3 $filename"), None);
        assert_eq!(command_override("  \n", "python3 $filename"), None);
        assert_eq!(command_override(" python3 $filename\n", "python3 $filename"), None);
        assert_eq!(command_override("python3 -X dev $filename", "python3 $filename"), Some("python3 -X dev $filename".into()));
    }

    #[test]
    fn compile_flags_can_be_cleared() {
        assert_eq!(flags_override("-std=c11", "-std=c11"), None);
        assert_eq!(flags_override("", "-std=c11"), Some(String::new()));
        assert_eq!(flags_override("-O2", ""), Some("-O2".into()));
        assert_eq!(flags_override("", ""), None);
    }

    #[test]
    fn environment_rows_become_variables() {
        let rows = [row(0, " DEBUG ", "1"), row(1, "", ""), row(2, "_PATH2", "a=b")];
        assert_eq!(environment(&rows).unwrap(), BTreeMap::from([("DEBUG".into(), "1".into()), ("_PATH2".into(), "a=b".into())]));
        assert!(environment(&[row(0, "", "orphan")]).is_err());
        assert!(environment(&[row(0, "2X", "1")]).is_err());
        assert!(environment(&[row(0, "A-B", "1")]).is_err());
        assert!(environment(&[row(0, "A", "1"), row(1, "A", "2")]).is_err());
    }

    #[test]
    fn custom_folders_are_absolute() {
        let home = Some(PathBuf::from("/Users/me"));
        assert_eq!(custom_folder("~/code", home.clone()).unwrap(), PathBuf::from("/Users/me/code"));
        assert_eq!(custom_folder("~", home.clone()).unwrap(), PathBuf::from("/Users/me"));
        assert_eq!(custom_folder(" /tmp ", home.clone()).unwrap(), PathBuf::from("/tmp"));
        assert!(custom_folder("code", home.clone()).is_err());
        assert!(custom_folder("~other/x", home.clone()).is_err());
        assert!(custom_folder("", home).is_err());
    }

    #[test]
    fn a_draft_round_trips_through_the_document() {
        let run = SavedRun {
            run_command: Some("python3 -X dev $filename".into()), arguments: "a 'b c'".into(), compiler_flags: None, input: "1 2\n".into(),
            environment: BTreeMap::from([("X".into(), "1".into())]), input_via_environment: true, working_folder: RunFolder::Custom("/tmp".into()),
        };
        let draft = Draft::of(&run, &python());
        assert_eq!(draft.folder, Folder::Custom);
        assert_eq!(settle(&draft, &python(), None).unwrap(), run);
    }

    #[test]
    fn an_untouched_draft_stays_default() {
        let draft = Draft::of(&SavedRun::default(), &c());
        assert_eq!(draft.run_command, "$compiler");
        assert_eq!(draft.compile_flags, "-std=c11");
        assert_eq!(settle(&draft, &c(), None).unwrap(), SavedRun::default());
        // Flags of a language without a compile step are never stored.
        let draft = Draft { compile_flags: "-O2".into(), ..Draft::of(&SavedRun::default(), &python()) };
        assert_eq!(settle(&draft, &python(), None).unwrap().compiler_flags, None);
    }

    #[test]
    fn problems_name_their_field() {
        let draft = Draft { arguments: "\"open".into(), environment: vec![row(0, "1A", "x")], folder: Folder::Custom, ..Draft::default() };
        let problems = settle(&draft, &python(), None).unwrap_err();
        let fields: Vec<_> = problems.iter().map(|p| p.field).collect();
        assert_eq!(fields, [Field::Arguments, Field::Environment, Field::Folder]);
    }

    #[test]
    fn the_run_uses_the_documents_settings() {
        let run = SavedRun {
            run_command: Some(" ".into()), arguments: "-v".into(), compiler_flags: Some(String::new()), input: "42".into(),
            environment: BTreeMap::from([("X".into(), "1".into())]), input_via_environment: true, working_folder: RunFolder::Home,
        };
        let settings = settings_for(&run);
        assert_eq!(settings.run_command, None);
        assert_eq!(settings.compile_flags, Some(String::new()));
        assert_eq!(settings.arguments, "-v");
        assert_eq!(settings.input, Some(ProgramInput::Text("42".into())));
        assert!(settings.input_via_environment);
        assert_eq!(settings.environment["X"], "1");
        assert_eq!(settings.working_directory, WorkingDirectory::Default);
        let custom = SavedRun { working_folder: RunFolder::Custom("/tmp".into()), ..SavedRun::default() };
        assert_eq!(settings_for(&custom).working_directory, WorkingDirectory::Custom("/tmp".into()));
        assert_eq!(settings_for(&SavedRun::default()).input, None);
        assert_eq!(settings_for(&SavedRun::default()).working_directory, WorkingDirectory::FileDirectory);
    }

    #[test]
    fn the_argument_preview_shows_the_split_list() {
        assert_eq!(argument_preview(r#"one "two words""#).as_deref(), Some("[one] [two words]"));
        assert_eq!(argument_preview("  "), None);
        assert_eq!(argument_preview("\"open"), None);
    }

    #[test]
    fn settle_clears_overrides_after_defaults_update() {
        let mut defaults = c();
        let draft = Draft {
            run_command: "$compiler --custom".into(),
            compile_flags: "-O3".into(),
            ..Draft::of(&SavedRun::default(), &defaults)
        };
        assert_eq!(settle(&draft, &defaults, None).unwrap().run_command.as_deref(), Some("$compiler --custom"));
        defaults.command = "$compiler --custom".into();
        defaults.flags = "-O3".into();
        let settled = settle(&draft, &defaults, None).unwrap();
        assert_eq!(settled.run_command, None);
        assert_eq!(settled.compiler_flags, None);
    }
}
