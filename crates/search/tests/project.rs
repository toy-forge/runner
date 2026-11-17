//! End-to-end tests of project search and replace on real temporary files.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use scratch_search::*;
use tempfile::TempDir;

fn write_bytes(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create dir");
    }
    fs::write(&path, bytes).expect("write fixture");
    path
}

fn write_str(root: &Path, rel: &str, text: &str) -> PathBuf {
    write_bytes(root, rel, text.as_bytes())
}

fn tempdir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn folder_request(root: &Path, pattern: &str, mode: SearchMode) -> SearchRequest {
    let matcher = Matcher::simple(pattern, MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(SearchSource::Folder(root.to_path_buf()), matcher);
    request.mode = mode;
    request
}

fn regex_request(root: &Path, pattern: &str, mode: SearchMode) -> SearchRequest {
    let matcher = Matcher::simple(pattern, MatchMode::Regex, true).expect("regex");
    let mut request = SearchRequest::new(SearchSource::Folder(root.to_path_buf()), matcher);
    request.mode = mode;
    request
}

fn match_at(results: &SearchResults, line: u32, column: u32) -> &ProjectMatch {
    results
        .matches
        .iter()
        .find(|m| m.line == line && m.column == column)
        .unwrap_or_else(|| panic!("no match at {line}:{column}: {:?}", results.matches))
}

#[test]
fn unicode_cjk_emoji_and_combining_columns() {
    let dir = tempdir();
    write_str(
        dir.path(),
        "a.txt",
        "one\ntwo needle 😀 needle\n日本語 😀 needle\ne\u{301}x needle\n",
    );
    let results = search_project(folder_request(dir.path(), "needle", SearchMode::Contents));
    assert_eq!(results.outcome, SearchOutcome::Completed);
    assert_eq!(results.matches.len(), 4);
    assert_eq!(results.files.len(), 1);
    assert!(!results.files[0].crlf);

    let first = match_at(&results, 1, 4);
    assert_eq!(first.line_text, "two needle 😀 needle");
    assert_eq!(first.length, 6);
    // "two needle 😀 " = 4 + 6 + 1 + 2 (surrogate pair) + 1
    let second = match_at(&results, 1, 14);
    assert_eq!(second.length, 6);
    // "日本語 " = 3 CJK + space, then "😀 " = 2 + 1
    match_at(&results, 2, 7);
    // "e" + combining acute + "x " -> the match is at UTF-16 column 4
    match_at(&results, 3, 4);

    // No normalisation: the precomposed query does not find the decomposed text.
    let composed = folder_request(dir.path(), "\u{e9}", SearchMode::Contents);
    assert!(search_project(composed).matches.is_empty());
}

#[test]
fn crlf_search_and_replace_preserves_line_endings() {
    let dir = tempdir();
    let path = write_str(dir.path(), "crlf.txt", "a needle\r\nb needle\r\n");
    let results = search_project(regex_request(dir.path(), "needle$", SearchMode::Contents));
    assert_eq!(results.matches.len(), 2);
    assert!(results.files[0].crlf);
    assert_eq!(results.matches[0].line_text, "a needle");

    let matcher = Matcher::simple("needle$", MatchMode::Regex, false).expect("matcher");
    let replacement = matcher.compile_replacement("pin").expect("replacement");
    let plan =
        ReplacePlan::new(&results, matcher, replacement, TargetSelection::All).expect("plan");
    let report = plan.execute(&ReplaceOptions::default());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(fs::read(&path).expect("read"), b"a pin\r\nb pin\r\n");

    // A replacement with a newline is written as CRLF.
    let dir = tempdir();
    let path = write_str(dir.path(), "crlf2.txt", "one\r\ntwo\r\n");
    let matcher = Matcher::simple("two", MatchMode::Text, true).expect("matcher");
    let results = search_project({
        let mut r = SearchRequest::new(
            SearchSource::Folder(dir.path().to_path_buf()),
            matcher.clone(),
        );
        r.mode = SearchMode::Contents;
        r
    });
    let replacement = matcher.compile_replacement("a\nb").expect("replacement");
    let plan =
        ReplacePlan::new(&results, matcher, replacement, TargetSelection::All).expect("plan");
    plan.execute(&ReplaceOptions::default());
    assert_eq!(fs::read(&path).expect("read"), b"one\r\na\r\nb\r\n");
}

#[test]
fn filename_modes() {
    let dir = tempdir();
    write_str(dir.path(), "needle.txt", "nothing here");
    let filenames = search_project(folder_request(dir.path(), "needle", SearchMode::Filenames));
    assert_eq!(filenames.matches.len(), 1);
    assert_eq!(filenames.matches[0].kind, MatchKind::FileName);
    assert_eq!(filenames.matches[0].line, 0);
    assert_eq!(filenames.matches[0].column, 0);
    assert_eq!(filenames.matches[0].length, 6);
    assert_eq!(filenames.matches[0].line_text, "needle.txt");

    let both = search_project(folder_request(
        dir.path(),
        "needle",
        SearchMode::ContentsAndFilenames,
    ));
    assert_eq!(both.matches.len(), 1);

    let contents = search_project(folder_request(dir.path(), "needle", SearchMode::Contents));
    assert!(contents.matches.is_empty());

    write_str(dir.path(), "needle2.txt", "a needle");
    let both = search_project(folder_request(
        dir.path(),
        "needle",
        SearchMode::ContentsAndFilenames,
    ));
    assert_eq!(both.matches.len(), 3);
    assert_eq!(
        both.matches
            .iter()
            .filter(|m| m.kind == MatchKind::FileName)
            .count(),
        2
    );
}

#[test]
fn filters_hidden_binary_subdirectories_and_exclusions() {
    let dir = tempdir();
    write_str(dir.path(), "src/a.rs", "needle");
    write_str(dir.path(), "src/b.txt", "needle");
    write_str(dir.path(), "node_modules/c.js", "needle");
    write_str(dir.path(), ".hidden/d.txt", "needle");
    write_bytes(dir.path(), "bin.dat", b"needle\0tail");
    write_str(dir.path(), "top.txt", "needle");
    write_str(dir.path(), "sub/deep.txt", "needle");

    let mut request = folder_request(dir.path(), "needle", SearchMode::Contents);
    request.options.include_extensions = vec!["rs".into()];
    request.options.excluded_directories = vec!["node_modules".into(), ".hidden".into()];
    let results = search_project(request);
    assert_eq!(results.matches.len(), 1);
    assert!(results.files[0].path.ends_with("src/a.rs"));

    let mut request = folder_request(dir.path(), "needle", SearchMode::Contents);
    request.options.exclude_extensions = vec!["rs".into()];
    request.options.excluded_directories = vec!["node_modules".into()];
    request.options.binary = true;
    let results = search_project(request);
    let names: Vec<_> = results.files.iter().map(|f| f.name.as_str()).collect();
    assert!(!names.contains(&"a.rs"));
    assert!(!names.contains(&"c.js"));
    assert!(names.contains(&"top.txt"));
    assert!(names.contains(&"b.txt"));
    assert!(names.contains(&"deep.txt"));
    assert!(names.contains(&"bin.dat"));
    assert!(
        results
            .files
            .iter()
            .any(|f| f.name == "bin.dat" && f.binary)
    );

    // Hidden files are off by default, on with the option.
    let results = search_project(folder_request(dir.path(), "needle", SearchMode::Contents));
    assert!(
        !results
            .files
            .iter()
            .any(|f| f.path.to_string_lossy().contains(".hidden"))
    );
    let mut request = folder_request(dir.path(), "needle", SearchMode::Contents);
    request.options.hidden = true;
    let results = search_project(request);
    assert!(
        results
            .files
            .iter()
            .any(|f| f.path.to_string_lossy().contains(".hidden"))
    );

    // Binary contents are skipped unless requested.
    let results = search_project(folder_request(dir.path(), "needle", SearchMode::Contents));
    assert!(!results.files.iter().any(|f| f.name == "bin.dat"));

    // "Search Subdirectories" off sees only the root's files.
    let mut request = folder_request(dir.path(), "needle", SearchMode::Contents);
    request.options.subdirectories = false;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.files[0].name, "top.txt");
}

#[test]
fn max_results_limits_and_cancellation() {
    let dir = tempdir();
    write_str(dir.path(), "many.txt", &"needle ".repeat(10));
    let mut request = folder_request(dir.path(), "needle", SearchMode::Contents);
    request.max_results = Some(3);
    let results = search_project(request);
    assert_eq!(results.matches.len(), 3);
    assert_eq!(results.outcome, SearchOutcome::LimitReached);
    assert!(results.truncated);

    // A pre-cancelled search returns immediately.
    let token = CancelToken::new();
    token.cancel();
    let results = search_project_with(
        folder_request(dir.path(), "needle", SearchMode::Contents),
        token,
    );
    assert_eq!(results.outcome, SearchOutcome::Cancelled);
    assert!(results.matches.is_empty());
}

#[test]
fn streaming_events_and_stable_ids() {
    let dir = tempdir();
    write_str(dir.path(), "a.txt", "one needle");
    write_str(dir.path(), "b.txt", "two needle\nthree needle");
    let handle = spawn_search(folder_request(dir.path(), "needle", SearchMode::Contents));
    let mut events = Vec::new();
    let mut done = None;
    loop {
        let batch = handle.recv_batch(16, Duration::from_millis(500));
        if batch.is_empty() {
            match handle.recv_timeout(Duration::from_secs(5)) {
                Ok(event) => {
                    if let SearchEvent::Done(d) = &event {
                        done = Some(d.clone());
                    }
                    events.push(event);
                }
                Err(_) => break,
            }
        } else {
            for event in batch {
                if let SearchEvent::Done(d) = &event {
                    done = Some(d.clone());
                }
                events.push(event);
            }
        }
        if done.is_some() {
            break;
        }
    }
    let results = handle.join();
    let done = done.expect("Done event");
    assert_eq!(done.outcome, SearchOutcome::Completed);
    assert_eq!(done.matches, 3);
    assert_eq!(results.matches.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, SearchEvent::FileStart { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, SearchEvent::Match { .. }))
            .count(),
        3
    );
    let mut ids = std::collections::HashSet::new();
    for m in &results.matches {
        assert!(ids.insert(m.id));
    }
    let first_ids: Vec<_> = results.files.iter().map(|f| f.id).collect();
    let again = search_project(folder_request(dir.path(), "needle", SearchMode::Contents));
    let second_ids: Vec<_> = again.files.iter().map(|f| f.id).collect();
    assert_eq!(first_ids, second_ids);
}

#[test]
fn cancellation_mid_walk() {
    let dir = tempdir();
    for i in 0..200 {
        write_str(
            dir.path(),
            &format!("big/{i}.txt"),
            &"filler line\n".repeat(8_000),
        );
    }
    let handle = spawn_search(folder_request(dir.path(), "needle", SearchMode::Contents));
    std::thread::sleep(Duration::from_millis(2));
    handle.cancel();
    let results = handle.join();
    assert_eq!(results.outcome, SearchOutcome::Cancelled);
}

#[test]
fn huge_file_performance() {
    let dir = tempdir();
    let mut text = String::with_capacity(6 * 1024 * 1024);
    for _ in 0..500_000 {
        text.push_str("filler line\n");
    }
    text.push_str("needle\n");
    write_str(dir.path(), "huge.txt", &text);
    let start = Instant::now();
    let results = search_project(folder_request(dir.path(), "needle", SearchMode::Contents));
    let elapsed = start.elapsed();
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].line, 500_000);
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

#[test]
fn selected_items_custom_paths_and_scope_filter() {
    let dir = tempdir();
    let a = write_str(dir.path(), "a.txt", "aa needle bb needle");
    let b = write_str(dir.path(), "b.txt", "needle");
    write_str(dir.path(), "c.txt", "needle");
    let matcher = Matcher::simple("needle", MatchMode::Text, true).expect("matcher");

    let mut request = SearchRequest::new(
        SearchSource::SelectedItems(vec![a.clone(), b.clone()]),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    assert_eq!(results.files.len(), 2);
    assert_eq!(results.matches.len(), 3);

    let mut request =
        SearchRequest::new(SearchSource::CustomPaths(vec![b.clone()]), matcher.clone());
    request.mode = SearchMode::Contents;
    assert_eq!(search_project(request).matches.len(), 1);

    let mut request = SearchRequest::new(SearchSource::Folder(dir.path().to_path_buf()), matcher);
    request.mode = SearchMode::Contents;
    request.options.scope_filter = Some(std::sync::Arc::new(|offset: usize| offset >= 10));
    let results = search_project(request);
    // Only a.txt's second occurrence ("aa needle bb needle") starts at 13.
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].byte_range.start, 13);
    assert!(results.matches.iter().all(|m| m.byte_range.start >= 10));
}

fn replace_plan(
    results: &SearchResults,
    matcher: Matcher,
    template: &str,
    targets: TargetSelection,
) -> ReplacePlan {
    let replacement = matcher.compile_replacement(template).expect("replacement");
    ReplacePlan::new(results, matcher, replacement, targets).expect("plan")
}

#[test]
fn replace_open_document_snapshot_and_per_match_flags() {
    let matcher = Matcher::simple("alpha", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::OpenDocuments(vec![DocumentSnapshot::new(
            "Untitled 1",
            None,
            "alpha beta\nalpha",
        )]),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 2);
    let mut plan = replace_plan(&results, matcher, "gamma", TargetSelection::All);
    assert_eq!(
        plan.summary(),
        ReplaceSummary {
            files: 1,
            matches: 2,
            filename_matches: 0
        }
    );

    let second = plan.files()[0].matches[1].id;
    assert!(plan.set_accepted(second, false));
    assert_eq!(plan.accepted_count(), 1);
    let dry = plan.execute(&ReplaceOptions {
        dry_run: true,
        ..Default::default()
    });
    assert!(dry.dry_run);
    assert!(dry.journal.is_none());
    assert_eq!(dry.files.len(), 1);
    assert!(!dry.files[0].written);
    assert_eq!(dry.files[0].replaced, 1);
    let edited = apply_edits("alpha beta\nalpha", &dry.files[0].edits).expect("apply");
    assert_eq!(edited, "gamma beta\nalpha");

    let real = plan.execute(&ReplaceOptions::default());
    assert!(real.journal.as_ref().is_some_and(|j| j.is_empty()));
    let edited = apply_edits("alpha beta\nalpha", &real.files[0].edits).expect("apply");
    assert_eq!(edited, "gamma beta\nalpha");
}

#[test]
fn replace_encoding_safe_and_undo() {
    let dir = tempdir();
    let path = write_bytes(dir.path(), "latin.txt", b"caf\xe9 and caf\xe9\n");
    let matcher = Matcher::simple("é", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 2);
    assert_eq!(
        results.files[0].encoding.expect("encoding").name(),
        "windows-1252"
    );

    let plan = replace_plan(&results, matcher, "è", TargetSelection::All);
    let dry = plan.execute(&ReplaceOptions {
        dry_run: true,
        ..Default::default()
    });
    assert!(!dry.files[0].written);
    assert_eq!(fs::read(&path).expect("read"), b"caf\xe9 and caf\xe9\n");

    let report = plan.execute(&ReplaceOptions::default());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert!(report.files[0].written);
    assert_eq!(fs::read(&path).expect("read"), b"caf\xe8 and caf\xe8\n");
    assert_eq!(report.replaced_matches(), 2);
    let journal = report.journal.expect("journal");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal.label(), "Undo Replace");
    let undo = journal.undo();
    assert_eq!(undo.restored, vec![path.clone()]);
    assert!(undo.failures.is_empty());
    assert_eq!(fs::read(&path).expect("read"), b"caf\xe9 and caf\xe9\n");
}

#[cfg(unix)]
#[test]
fn content_replace_preserves_attributes_and_refuses_symlink_replacement() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = tempdir();
    let path = write_str(dir.path(), "source.txt", "needle");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("permissions");
    xattr::set(&path, "user.scratchrunner-test", b"metadata").expect("set xattr");
    let matcher = Matcher::simple("needle", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::SelectedItems(vec![path.clone()]),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let plan = replace_plan(
        &search_project(request),
        matcher.clone(),
        "pin",
        TargetSelection::All,
    );
    let report = plan.execute(&ReplaceOptions::default());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(fs::read(&path).expect("read"), b"pin");
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        xattr::get(&path, "user.scratchrunner-test").expect("get xattr"),
        Some(b"metadata".to_vec())
    );
    let undo = report.journal.expect("journal").undo();
    assert!(undo.failures.is_empty(), "{:?}", undo.failures);
    assert_eq!(
        xattr::get(&path, "user.scratchrunner-test").expect("get xattr"),
        Some(b"metadata".to_vec())
    );

    let link = dir.path().join("link.txt");
    symlink(&path, &link).expect("symlink");
    let mut request = SearchRequest::new(
        SearchSource::SelectedItems(vec![link.clone()]),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let plan = replace_plan(
        &search_project(request),
        matcher,
        "pin",
        TargetSelection::All,
    );
    let report = plan.execute(&ReplaceOptions::default());
    assert!(
        !report.issues.is_empty(),
        "symlink replace must be rejected"
    );
    assert!(
        fs::symlink_metadata(&link)
            .expect("link")
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&path).expect("source"), b"needle");
}

#[test]
fn replace_detects_changed_content_and_encoding() {
    let dir = tempdir();
    let path = write_str(dir.path(), "change.txt", "needle\n");
    let matcher = Matcher::simple("needle", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    let plan = replace_plan(&results, matcher.clone(), "pin", TargetSelection::All);

    fs::write(&path, "needle changed\n").expect("modify");
    let report = plan.execute(&ReplaceOptions::default());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].kind, ReplaceIssueKind::Changed);
    assert_eq!(fs::read(&path).expect("read"), b"needle changed\n");

    // Encoding change between search and replace.
    let dir = tempdir();
    let path = write_str(dir.path(), "enc.txt", "needle\n");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    let plan = replace_plan(&results, matcher, "pin", TargetSelection::All);
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in "needle\n".encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(&path, &utf16).expect("rewrite utf16");
    let report = plan.execute(&ReplaceOptions::default());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].kind, ReplaceIssueKind::EncodingChanged);
    assert_eq!(fs::read(&path).expect("read"), utf16);
}

#[test]
fn replace_per_match_flags_on_disk() {
    let dir = tempdir();
    let path = write_str(dir.path(), "flags.txt", "aa aa aa");
    let matcher = Matcher::simple("aa", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 3);
    let mut plan = replace_plan(&results, matcher, "bb", TargetSelection::All);
    let ids: Vec<_> = plan.files()[0].matches.iter().map(|m| m.id).collect();
    assert!(plan.set_accepted(ids[1], false));
    let report = plan.execute(&ReplaceOptions::default());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(fs::read(&path).expect("read"), b"bb aa bb");

    // Selecting only one match by id.
    let dir = tempdir();
    let path = write_str(dir.path(), "one.txt", "x x x");
    let matcher = Matcher::simple("x", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    let second = results.matches[1].id;
    let plan = replace_plan(
        &results,
        matcher,
        "y",
        TargetSelection::Matches(vec![second]),
    );
    assert_eq!(plan.summary().matches, 1);
    plan.execute(&ReplaceOptions::default());
    assert_eq!(fs::read(&path).expect("read"), b"x y x");
}

#[test]
fn filename_replace_renames_with_conflicts_and_undo() {
    let dir = tempdir();
    let foo = write_str(dir.path(), "foo.txt", "x");
    let matcher = Matcher::simple("foo", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Filenames;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].kind, MatchKind::FileName);
    let plan = replace_plan(&results, matcher, "bar", TargetSelection::All);
    assert_eq!(plan.summary().filename_matches, 1);
    assert!(plan.rename_conflicts().is_empty());

    let bar = write_str(dir.path(), "bar.txt", "y");
    let conflicts = plan.rename_conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].to, bar);

    // Skip keeps the old name.
    let report = plan.execute(&ReplaceOptions {
        conflicts: ConflictPolicy::Skip,
        ..Default::default()
    });
    assert_eq!(report.renames.len(), 1);
    assert_eq!(report.renames[0].status, RenameStatus::Skipped);
    assert!(foo.exists());

    // Undo restores both the renamed file and the replaced target.
    let report = plan.execute(&ReplaceOptions {
        conflicts: ConflictPolicy::Overwrite,
        ..Default::default()
    });
    assert_eq!(report.renames[0].status, RenameStatus::Renamed);
    assert!(!foo.exists());
    assert_eq!(fs::read(&bar).expect("read"), b"x");
    let undo = report.journal.expect("journal").undo();
    assert_eq!(undo.renamed.len(), 1);
    assert!(foo.exists());
    assert_eq!(fs::read(&foo).expect("read"), b"x");
    assert_eq!(fs::read(&bar).expect("read"), b"y");
}

#[test]
fn overwrite_undo_keeps_recovery_steps_until_conflict_is_resolved() {
    let dir = tempdir();
    let foo = write_str(dir.path(), "foo.txt", "source");
    let bar = write_str(dir.path(), "bar.txt", "original target");
    let matcher = Matcher::simple("foo", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Filenames;
    let results = search_project(request);
    let report =
        replace_plan(&results, matcher, "bar", TargetSelection::All).execute(&ReplaceOptions {
            conflicts: ConflictPolicy::Overwrite,
            ..Default::default()
        });
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let journal = report.journal.expect("journal");
    assert_eq!(journal.len(), 2);
    let backup = match &journal.entries()[0] {
        JournalEntry::RestoreReplacedFile { backup, .. } => backup.clone(),
        entry => panic!("expected backup, got {entry:?}"),
    };
    assert_eq!(fs::read(&backup).expect("backup"), b"original target");

    // Someone has reused the original source name. Undo must not overwrite
    // that file or discard the target's only recovery copy.
    fs::write(&foo, "new file").expect("new file");
    let undo = journal.undo();
    assert_eq!(undo.failures.len(), 1);
    assert_eq!(undo.remaining.len(), 2);
    assert_eq!(fs::read(&foo).expect("new file"), b"new file");
    assert_eq!(fs::read(&bar).expect("renamed source"), b"source");
    assert_eq!(fs::read(&backup).expect("backup"), b"original target");

    fs::remove_file(&foo).expect("clear conflict");
    let retry = undo.remaining.undo();
    assert!(retry.failures.is_empty(), "{:?}", retry.failures);
    assert!(retry.remaining.is_empty());
    assert_eq!(fs::read(&foo).expect("restored source"), b"source");
    assert_eq!(fs::read(&bar).expect("restored target"), b"original target");
    assert!(!backup.exists());
}

#[test]
fn cancelled_undo_can_be_retried() {
    let dir = tempdir();
    let foo = write_str(dir.path(), "foo.txt", "source");
    let bar = write_str(dir.path(), "bar.txt", "target");
    let matcher = Matcher::simple("foo", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Filenames;
    let report = replace_plan(
        &search_project(request),
        matcher,
        "bar",
        TargetSelection::All,
    )
    .execute(&ReplaceOptions {
        conflicts: ConflictPolicy::Overwrite,
        ..Default::default()
    });
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let token = CancelToken::new();
    token.cancel();
    let undo = report.journal.expect("journal").undo_with(&token);
    assert!(undo.cancelled);
    assert_eq!(undo.remaining.len(), 2);
    let retry = undo.remaining.undo();
    assert!(retry.failures.is_empty(), "{:?}", retry.failures);
    assert_eq!(fs::read(foo).expect("source"), b"source");
    assert_eq!(fs::read(bar).expect("target"), b"target");
}

#[cfg(unix)]
#[test]
fn overwrite_undo_preserves_target_metadata_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = tempdir();
    let foo = write_str(dir.path(), "foo.txt", "source");
    let bar = write_str(dir.path(), "bar.txt", "target");
    fs::set_permissions(&bar, fs::Permissions::from_mode(0o640)).expect("permissions");
    xattr::set(&bar, "user.scratchrunner-test", b"metadata").expect("set xattr");
    let before = fs::metadata(&bar).expect("metadata");
    let matcher = Matcher::simple("foo", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Filenames;
    let plan = replace_plan(
        &search_project(request),
        matcher,
        "bar",
        TargetSelection::All,
    );
    let options = ReplaceOptions {
        conflicts: ConflictPolicy::Overwrite,
        ..Default::default()
    };
    let report = plan.execute(&options);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let undo = report.journal.expect("journal").undo();
    assert!(undo.failures.is_empty(), "{:?}", undo.failures);
    let after = fs::metadata(&bar).expect("metadata");
    assert_eq!(after.permissions().mode(), before.permissions().mode());
    assert_eq!(
        after.modified().expect("mtime"),
        before.modified().expect("mtime")
    );
    assert_eq!(
        xattr::get(&bar, "user.scratchrunner-test").expect("get xattr"),
        Some(b"metadata".to_vec())
    );

    fs::remove_file(&bar).expect("remove target");
    symlink(&foo, &bar).expect("create symlink");
    assert_eq!(plan.rename_conflicts().len(), 1);
    let report = plan.execute(&options);
    assert_eq!(report.renames[0].status, RenameStatus::Failed);
    assert!(
        fs::symlink_metadata(&bar)
            .expect("symlink")
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&foo).expect("source"), b"source");
}

#[test]
fn filename_replace_applies_all_matches_in_a_name() {
    let dir = tempdir();
    let path = write_str(dir.path(), "foo_foo.txt", "");
    let matcher = Matcher::simple("foo", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Filenames;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 2);
    let plan = replace_plan(&results, matcher, "bar", TargetSelection::All);
    let report = plan.execute(&ReplaceOptions::default());
    assert_eq!(report.renames.len(), 1);
    let renamed = report.renames[0].to.clone();
    assert_eq!(
        renamed.file_name().and_then(|n| n.to_str()),
        Some("bar_bar.txt")
    );
    assert!(!path.exists());
    assert!(renamed.exists());
    let undo = report.journal.expect("journal").undo();
    assert_eq!(undo.renamed.len(), 1);
    assert!(path.exists());
}

#[test]
fn contents_and_filename_replace_together_undoes_in_reverse() {
    let dir = tempdir();
    let foo = write_str(dir.path(), "foo.txt", "foo");
    let matcher = Matcher::simple("foo", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::ContentsAndFilenames;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 2);
    let plan = replace_plan(&results, matcher, "bar", TargetSelection::All);
    let report = plan.execute(&ReplaceOptions::default());
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let bar = dir.path().join("bar.txt");
    assert!(!foo.exists());
    assert_eq!(fs::read(&bar).expect("read"), b"bar");
    let journal = report.journal.expect("journal");
    assert_eq!(journal.len(), 2); // write + rename
    let undo = journal.undo();
    assert!(undo.failures.is_empty(), "{:?}", undo.failures);
    assert!(foo.exists());
    assert_eq!(fs::read(&foo).expect("read"), b"foo");
    assert!(!bar.exists());
}

#[test]
fn plan_errors_for_unknown_targets() {
    let dir = tempdir();
    write_str(dir.path(), "a.txt", "needle");
    let matcher = Matcher::simple("needle", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Contents;
    let results = search_project(request);
    let replacement = matcher.compile_replacement("x").expect("replacement");
    let unknown_match = MatchId {
        file: FileId(1),
        index: 9,
    };
    assert_eq!(
        ReplacePlan::new(
            &results,
            matcher.clone(),
            replacement.clone(),
            TargetSelection::Matches(vec![unknown_match])
        )
        .unwrap_err(),
        ReplacePlanError::UnknownMatch(unknown_match)
    );
    assert_eq!(
        ReplacePlan::new(
            &results,
            matcher,
            replacement,
            TargetSelection::Files(vec![FileId(42)])
        )
        .unwrap_err(),
        ReplacePlanError::UnknownFile(FileId(42))
    );
}

#[test]
fn filename_replace_renames_folders() {
    let dir = tempdir();
    let old = dir.path().join("olddir");
    fs::create_dir(&old).expect("mkdir");
    let file = old.join("keep.txt");
    fs::write(&file, "keep").expect("write");

    let matcher = Matcher::simple("olddir", MatchMode::Text, true).expect("matcher");
    let mut request = SearchRequest::new(
        SearchSource::Folder(dir.path().to_path_buf()),
        matcher.clone(),
    );
    request.mode = SearchMode::Filenames;
    let results = search_project(request);
    assert_eq!(results.matches.len(), 1);
    assert_eq!(results.matches[0].kind, MatchKind::FileName);
    assert!(results.files[0].is_dir);

    let plan = replace_plan(&results, matcher, "newdir", TargetSelection::All);
    let report = plan.execute(&ReplaceOptions::default());
    let new = dir.path().join("newdir");
    assert!(new.is_dir());
    assert!(!old.exists());
    assert_eq!(fs::read(new.join("keep.txt")).expect("read"), b"keep");
    let undo = report.journal.expect("journal").undo();
    assert_eq!(undo.renamed.len(), 1);
    assert!(old.is_dir());
    assert_eq!(fs::read(old.join("keep.txt")).expect("read"), b"keep");
}

#[test]
fn search_mode_labels_and_source_roots() {
    assert_eq!(
        SearchMode::ContentsAndFilenames.label(),
        "File Contents + Filenames"
    );
    assert_eq!(SearchMode::Filenames.label(), "Filenames");
    let source = SearchSource::CustomPaths(vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(source.roots().len(), 2);
    assert!(SearchSource::OpenDocuments(Vec::new()).roots().is_empty());
}
