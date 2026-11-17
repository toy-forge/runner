//! Project replace: turn search results into a reviewable [`ReplacePlan`],
//! then execute it (or dry-run it) with encoding-safe writes, filename
//! renames, conflict handling and an [`ReplaceJournal`] that powers "Undo
//! Replace".
//!
//! ```no_run
//! use scratch_search::*;
//! let matcher = Matcher::simple("cat", MatchMode::Text, true)?;
//! let mut request = SearchRequest::new(
//!     SearchSource::Folder("/tmp/project".into()),
//!     matcher.clone(),
//! );
//! request.mode = SearchMode::ContentsAndFilenames;
//! let results = search_project(request);
//! let replacement = matcher.compile_replacement("dog")?;
//! let plan = ReplacePlan::new(&results, matcher, replacement, TargetSelection::All)?;
//! let report = plan.execute(&ReplaceOptions { dry_run: true, ..Default::default() });
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Everything is verified against the search's fingerprints before a write:
//! raw-byte hash, detected encoding/BOM and (for open documents) text hash.
//! A file that changed since the search is skipped with
//! [`ReplaceIssueKind::Changed`] or [`ReplaceIssueKind::EncodingChanged`],
//! mirroring CodeRunner's "File could not be read for replace operation using
//! the same encoding" alert.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::cancel::CancelToken;
use crate::edit::{EditError, TextEdit, apply_edits, validate_edits};
use crate::matcher::Matcher;
use crate::project::encoding::{EncodeError, TextEncoding, content_hash, decode, encode};
use crate::project::search::{FileId, MatchId, MatchKind, ProjectMatch, SearchFile, SearchResults};
use crate::replacement::Replacement;

/// Which matches of the search a plan starts with. Per-match `accepted` flags
/// can be toggled afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TargetSelection {
    /// Every match.
    #[default]
    All,
    /// The listed matches ("Replace 1 Result", "Replace %lu Results").
    Matches(Vec<MatchId>),
    /// Every match in the listed files ("Replace Selected").
    Files(Vec<FileId>),
}

/// Why a plan could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReplacePlanError {
    /// A requested match id is not in the search results.
    #[error("the search results contain no match {0:?}")]
    UnknownMatch(MatchId),
    /// A requested file id is not in the search results.
    #[error("the search results contain no file {0:?}")]
    UnknownFile(FileId),
}

/// One match of a plan; `accepted` is the checkbox / selection state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMatch {
    /// Stable id from the search.
    pub id: MatchId,
    /// Contents or file name.
    pub kind: MatchKind,
    /// 0-based line (0 for filename matches).
    pub line: u32,
    /// UTF-16 column.
    pub column: u32,
    /// UTF-16 length.
    pub length: u32,
    /// Byte range in the decoded file text (or in the file name).
    pub byte_range: Range<usize>,
    /// The line shown in the results outline.
    pub line_text: String,
    /// Whether this match will be replaced.
    pub accepted: bool,
}

/// The matches of one file in a plan.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// The search's file record (encoding, hashes, snapshot).
    pub file: SearchFile,
    /// Its planned matches.
    pub matches: Vec<PlannedMatch>,
}

impl PlannedFile {
    /// Number of accepted matches.
    pub fn accepted(&self) -> usize {
        self.matches.iter().filter(|m| m.accepted).count()
    }
}

/// What a plan will do, for the confirmation UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReplaceSummary {
    /// Files with at least one accepted match.
    pub files: usize,
    /// Accepted matches.
    pub matches: usize,
    /// Accepted filename matches (`Filenames` mode renames).
    pub filename_matches: usize,
}

/// A file that would be renamed onto an existing path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameConflict {
    /// The file.
    pub file: FileId,
    /// Its current path.
    pub from: PathBuf,
    /// The planned new path.
    pub to: PathBuf,
}

/// A per-match toggleable replace plan.
#[derive(Debug, Clone)]
pub struct ReplacePlan {
    files: Vec<PlannedFile>,
    matcher: Matcher,
    replacement: Replacement,
    fallback: &'static encoding_rs::Encoding,
}

impl ReplacePlan {
    /// Builds a plan from search results. The matcher and replacement must be
    /// the ones the search used (the matcher is re-run to expand capture
    /// groups when the plan is executed).
    pub fn new(
        results: &SearchResults,
        matcher: Matcher,
        replacement: Replacement,
        targets: TargetSelection,
    ) -> Result<Self, ReplacePlanError> {
        let mut by_file: HashMap<FileId, PlannedFile> = HashMap::new();
        let mut order: Vec<FileId> = Vec::new();
        let selected_matches: Option<HashSet<MatchId>> = match &targets {
            TargetSelection::All => None,
            TargetSelection::Matches(ids) => {
                let known: HashSet<MatchId> = results.matches.iter().map(|m| m.id).collect();
                if let Some(missing) = ids.iter().find(|id| !known.contains(id)) {
                    return Err(ReplacePlanError::UnknownMatch(*missing));
                }
                Some(ids.iter().copied().collect())
            }
            TargetSelection::Files(_) => None,
        };
        let selected_files: Option<HashSet<FileId>> = match &targets {
            TargetSelection::Files(ids) => {
                let known: HashSet<FileId> = results.files.iter().map(|f| f.id).collect();
                if let Some(missing) = ids.iter().find(|id| !known.contains(id)) {
                    return Err(ReplacePlanError::UnknownFile(*missing));
                }
                Some(ids.iter().copied().collect())
            }
            _ => None,
        };

        for m in &results.matches {
            if let Some(ids) = &selected_matches
                && !ids.contains(&m.id)
            {
                continue;
            }
            if let Some(ids) = &selected_files
                && !ids.contains(&m.file)
            {
                continue;
            }
            let Some(file) = results.file(m.file) else {
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = by_file.entry(m.file) {
                order.push(m.file);
                entry.insert(PlannedFile {
                    file: file.clone(),
                    matches: Vec::new(),
                });
            }
            if let Some(entry) = by_file.get_mut(&m.file) {
                entry.matches.push(planned_match(m));
            }
        }

        let mut files: Vec<PlannedFile> = order
            .into_iter()
            .filter_map(|id| by_file.remove(&id))
            .collect();
        for file in &mut files {
            file.matches
                .sort_by_key(|m| (m.kind, m.line, m.column, m.id.index));
        }
        Ok(Self {
            files,
            matcher,
            replacement,
            fallback: results.fallback,
        })
    }

    /// The planned files.
    pub fn files(&self) -> &[PlannedFile] {
        &self.files
    }

    /// The replacement template.
    pub fn replacement(&self) -> &Replacement {
        &self.replacement
    }

    /// The matcher.
    pub fn matcher(&self) -> &Matcher {
        &self.matcher
    }

    /// All planned matches, file by file.
    pub fn matches(&self) -> impl Iterator<Item = &PlannedMatch> {
        self.files.iter().flat_map(|f| f.matches.iter())
    }

    /// Number of accepted matches.
    pub fn accepted_count(&self) -> usize {
        self.matches().filter(|m| m.accepted).count()
    }

    /// What executing this plan will do.
    pub fn summary(&self) -> ReplaceSummary {
        let mut summary = ReplaceSummary::default();
        for file in &self.files {
            let accepted: Vec<&PlannedMatch> = file.matches.iter().filter(|m| m.accepted).collect();
            if accepted.is_empty() {
                continue;
            }
            summary.files += 1;
            summary.matches += accepted.len();
            summary.filename_matches += accepted
                .iter()
                .filter(|m| m.kind == MatchKind::FileName)
                .count();
        }
        summary
    }

    /// Toggles one match. Returns whether the id was part of the plan.
    pub fn set_accepted(&mut self, id: MatchId, accepted: bool) -> bool {
        for file in &mut self.files {
            for m in &mut file.matches {
                if m.id == id {
                    m.accepted = accepted;
                    return true;
                }
            }
        }
        false
    }

    /// Accepts or rejects every match of one file. Returns the number of
    /// matches changed.
    pub fn set_file_accepted(&mut self, file: FileId, accepted: bool) -> usize {
        let mut changed = 0;
        for entry in &mut self.files {
            if entry.file.id == file {
                for m in &mut entry.matches {
                    if m.accepted != accepted {
                        changed += 1;
                        m.accepted = accepted;
                    }
                }
            }
        }
        changed
    }

    /// Accepts every match.
    pub fn accept_all(&mut self) {
        for file in &mut self.files {
            for m in &mut file.matches {
                m.accepted = true;
            }
        }
    }

    /// Rejects every match.
    pub fn reject_all(&mut self) {
        for file in &mut self.files {
            for m in &mut file.matches {
                m.accepted = false;
            }
        }
    }

    /// The rename targets that already exist on disk (the "A file with the
    /// name ... already exists" prompt). Only filename matches are considered.
    pub fn rename_conflicts(&self) -> Vec<RenameConflict> {
        let mut out = Vec::new();
        for file in &self.files {
            let Ok(Some(name)) = self.planned_name(file) else {
                continue;
            };
            let Some(parent) = file.file.path.parent() else {
                continue;
            };
            let to = parent.join(&name);
            if to == file.file.path || fs::symlink_metadata(&to).is_err() {
                continue;
            }
            if !same_nonsymlink_path(&file.file.path, &to) {
                out.push(RenameConflict {
                    file: file.file.id,
                    from: file.file.path.clone(),
                    to,
                });
            }
        }
        out
    }

    /// Executes the plan. With [`ReplaceOptions::dry_run`] nothing is written
    /// and no journal is produced.
    pub fn execute(&self, options: &ReplaceOptions) -> ReplaceReport {
        let mut report = ReplaceReport {
            dry_run: options.dry_run,
            ..ReplaceReport::default()
        };
        let mut journal = ReplaceJournal::new();
        let mut renames: Vec<PendingRename> = Vec::new();

        for planned in &self.files {
            if options.cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            let accepted: Vec<&PlannedMatch> =
                planned.matches.iter().filter(|m| m.accepted).collect();
            if accepted.is_empty() {
                continue;
            }

            // The file name part first (needs no file contents).
            let name_plan = match self.planned_name(planned) {
                Ok(plan) => plan,
                Err(kind) => {
                    report.issues.push(issue(planned, kind));
                    None
                }
            };
            if name_plan.is_some() && planned.file.snapshot.is_some() {
                report
                    .issues
                    .push(issue(planned, ReplaceIssueKind::SnapshotRename));
            }

            let content_matches: Vec<&&PlannedMatch> = accepted
                .iter()
                .filter(|m| m.kind == MatchKind::Content)
                .collect();

            let (text, raw, on_disk) = if planned.file.is_dir {
                match fs::metadata(&planned.file.path) {
                    Ok(meta) if meta.is_dir() => (String::new(), Vec::new(), true),
                    _ => {
                        report
                            .issues
                            .push(issue(planned, ReplaceIssueKind::Changed));
                        continue;
                    }
                }
            } else if let Some(snapshot) = &planned.file.snapshot {
                let text = snapshot.to_string();
                if content_hash(text.as_bytes()) != planned.file.text_hash {
                    report
                        .issues
                        .push(issue(planned, ReplaceIssueKind::Changed));
                    continue;
                }
                (text, Vec::new(), false)
            } else {
                match fs::read(&planned.file.path) {
                    Ok(bytes) => {
                        let decoded = decode(&bytes, self.fallback);
                        let recorded = planned.file.encoding;
                        if recorded.is_some_and(|e| e != decoded.encoding) {
                            report
                                .issues
                                .push(issue(planned, ReplaceIssueKind::EncodingChanged));
                            continue;
                        }
                        if content_hash(&bytes) != planned.file.byte_hash {
                            report
                                .issues
                                .push(issue(planned, ReplaceIssueKind::Changed));
                            continue;
                        }
                        if decoded.lossy {
                            report.issues.push(issue(planned, ReplaceIssueKind::Lossy));
                            continue;
                        }
                        if planned.file.binary && !options.replace_binary {
                            report.issues.push(issue(planned, ReplaceIssueKind::Binary));
                            continue;
                        }
                        let text = decoded.text;
                        (text, bytes, true)
                    }
                    Err(err) => {
                        report
                            .issues
                            .push(issue(planned, ReplaceIssueKind::Read(err.to_string())));
                        continue;
                    }
                }
            };

            let mut edits: Vec<TextEdit> = Vec::new();
            let mut replaced = 0usize;
            if !content_matches.is_empty() {
                let matcher = self.matcher.with_crlf(planned.file.crlf);
                for m in &content_matches {
                    match matcher.replacement_for(&text, &m.byte_range, &self.replacement) {
                        Ok(Some(edit)) => {
                            edits.push(edit);
                            replaced += 1;
                        }
                        Ok(None) => report
                            .issues
                            .push(issue(planned, ReplaceIssueKind::StaleMatch)),
                        Err(err) => report
                            .issues
                            .push(issue(planned, ReplaceIssueKind::Match(err.to_string()))),
                    }
                }
                edits.sort_by_key(|e| (e.range.start, e.range.end));
                if let Err(err) = validate_edits(&text, &edits) {
                    let kind = match err {
                        EditError::Overlap(..) => ReplaceIssueKind::Overlap,
                        EditError::OutOfRange(..) => ReplaceIssueKind::StaleMatch,
                    };
                    report.issues.push(issue(planned, kind));
                    continue;
                }
            }

            let new_text = match apply_edits(&text, &edits) {
                Ok(new_text) => new_text,
                Err(_) => {
                    report
                        .issues
                        .push(issue(planned, ReplaceIssueKind::Overlap));
                    continue;
                }
            };
            let content_changed = new_text != text;

            let mut written = false;
            if content_changed && on_disk && !options.dry_run {
                let Some(encoding) = planned.file.encoding else {
                    report
                        .issues
                        .push(issue(planned, ReplaceIssueKind::Changed));
                    continue;
                };
                let bytes = match encode(&new_text, encoding) {
                    Ok(bytes) => bytes,
                    Err(EncodeError::Unmappable(name)) => {
                        report.issues.push(issue(
                            planned,
                            ReplaceIssueKind::Unmappable(name.to_string()),
                        ));
                        continue;
                    }
                    Err(EncodeError::Unsupported(name)) => {
                        report.issues.push(issue(
                            planned,
                            ReplaceIssueKind::Unmappable(name.to_string()),
                        ));
                        continue;
                    }
                };
                if let Err(err) = write_atomic(&planned.file.path, &bytes, content_hash(&raw)) {
                    report
                        .issues
                        .push(issue(planned, ReplaceIssueKind::Write(err.to_string())));
                    continue;
                }
                written = true;
                journal.push(JournalEntry::Write {
                    path: planned.file.path.clone(),
                    before: raw,
                    after_hash: content_hash(&bytes),
                    encoding,
                });
            }

            if content_changed || name_plan.is_some() {
                let mut output = FileReplaceOutcome {
                    file: planned.file.id,
                    path: planned.file.path.clone(),
                    snapshot: planned.file.snapshot.is_some(),
                    replaced,
                    edits: edits.clone(),
                    written,
                    renamed_to: None,
                };
                if let Some(new_name) = name_plan {
                    if planned.file.snapshot.is_some() {
                        // Already reported; do not rename a buffer.
                    } else if new_name.is_empty()
                        || new_name.contains('/')
                        || new_name == "."
                        || new_name == ".."
                    {
                        report
                            .issues
                            .push(issue(planned, ReplaceIssueKind::InvalidName));
                    } else if new_name != planned.file.name {
                        let parent = planned.file.path.parent().unwrap_or(Path::new("."));
                        output.renamed_to = Some(parent.join(&new_name));
                        renames.push(PendingRename {
                            file: planned.file.id,
                            from: planned.file.path.clone(),
                            to: parent.join(new_name),
                        });
                    }
                }
                report.files.push(output);
            }
        }

        // Renames run after content writes, so a write never follows a path
        // change.
        let mut targets: HashSet<PathBuf> = HashSet::new();
        for pending in renames {
            if options.cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            if !targets.insert(pending.to.clone()) {
                report.issues.push(ReplaceIssue {
                    file: Some(pending.file),
                    path: Some(pending.from.clone()),
                    kind: ReplaceIssueKind::RenameConflict,
                });
                report.renames.push(RenameOutcome {
                    file: pending.file,
                    from: pending.from,
                    to: pending.to,
                    status: RenameStatus::Skipped,
                });
                continue;
            }
            let status = if options.dry_run {
                RenameStatus::DryRun
            } else {
                match rename_path(&pending.from, &pending.to, options.conflicts, &mut journal) {
                    Ok(true) => RenameStatus::Renamed,
                    Ok(false) => RenameStatus::Skipped,
                    Err(err) => {
                        report.issues.push(ReplaceIssue {
                            file: Some(pending.file),
                            path: Some(pending.from.clone()),
                            kind: ReplaceIssueKind::Write(err),
                        });
                        RenameStatus::Failed
                    }
                }
            };
            report.renames.push(RenameOutcome {
                file: pending.file,
                from: pending.from,
                to: pending.to,
                status,
            });
        }

        report.journal = (!options.dry_run).then_some(journal);
        report
    }

    /// The new file name from the accepted filename matches, or `None` when
    /// there are none.
    fn planned_name(&self, planned: &PlannedFile) -> Result<Option<String>, ReplaceIssueKind> {
        let name = &planned.file.name;
        let mut edits = Vec::new();
        for m in planned
            .matches
            .iter()
            .filter(|m| m.kind == MatchKind::FileName && m.accepted)
        {
            match self
                .matcher
                .replacement_for(name, &m.byte_range, &self.replacement)
            {
                Ok(Some(edit)) => edits.push(edit),
                Ok(None) => return Err(ReplaceIssueKind::StaleMatch),
                Err(err) => return Err(ReplaceIssueKind::Match(err.to_string())),
            }
        }
        if edits.is_empty() {
            return Ok(None);
        }
        edits.sort_by_key(|e| (e.range.start, e.range.end));
        validate_edits(name, &edits).map_err(|err| match err {
            EditError::Overlap(..) => ReplaceIssueKind::Overlap,
            EditError::OutOfRange(..) => ReplaceIssueKind::StaleMatch,
        })?;
        apply_edits(name, &edits)
            .map(Some)
            .map_err(|_| ReplaceIssueKind::Overlap)
    }
}

fn planned_match(m: &ProjectMatch) -> PlannedMatch {
    PlannedMatch {
        id: m.id,
        kind: m.kind,
        line: m.line,
        column: m.column,
        length: m.length,
        byte_range: m.byte_range.clone(),
        line_text: m.line_text.clone(),
        accepted: true,
    }
}

fn issue(planned: &PlannedFile, kind: ReplaceIssueKind) -> ReplaceIssue {
    ReplaceIssue {
        file: Some(planned.file.id),
        path: Some(planned.file.path.clone()),
        kind,
    }
}

/// How to treat a rename whose target already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// Leave the file under its old name and report it.
    #[default]
    Skip,
    /// Replace an existing regular file ("Ask before overwriting existing
    /// files" = off / the user answered Replace).
    Overwrite,
    /// Report the conflict as a failure and keep both files.
    Fail,
}

/// Options of [`ReplacePlan::execute`].
#[derive(Clone)]
pub struct ReplaceOptions {
    /// Compute everything but write and rename nothing; no journal.
    pub dry_run: bool,
    /// Rename conflict policy.
    pub conflicts: ConflictPolicy,
    /// Allow replacing files that sniffed as binary (off by default: a lossy
    /// byte-level rewrite is rarely what the user wants).
    pub replace_binary: bool,
    /// Stop between files.
    pub cancel: CancelToken,
}

impl Default for ReplaceOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            conflicts: ConflictPolicy::Skip,
            replace_binary: false,
            cancel: CancelToken::new(),
        }
    }
}

impl std::fmt::Debug for ReplaceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplaceOptions")
            .field("dry_run", &self.dry_run)
            .field("conflicts", &self.conflicts)
            .field("replace_binary", &self.replace_binary)
            .field("cancel", &self.cancel)
            .finish()
    }
}

/// Why one file or match was not replaced.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReplaceIssueKind {
    /// The file changed since the search.
    #[error("the file changed since the search")]
    Changed,
    /// The file's encoding changed since the search.
    #[error("the file encoding changed since the search")]
    EncodingChanged,
    /// The file has undecodable bytes.
    #[error("the file contains undecodable bytes")]
    Lossy,
    /// Binary files are not replaced by default.
    #[error("binary file contents are not replaced by default")]
    Binary,
    /// Two replacements overlap.
    #[error("two replacements overlap")]
    Overlap,
    /// The match is no longer a match at that position.
    #[error("the match is no longer valid at that position")]
    StaleMatch,
    /// The new text cannot be represented in the file encoding.
    #[error("the replacement cannot be represented in {0}")]
    Unmappable(String),
    /// Reading failed.
    #[error("could not read the file: {0}")]
    Read(String),
    /// Writing or renaming failed.
    #[error("could not write the file: {0}")]
    Write(String),
    /// The rename target already exists.
    #[error("a file with the new name already exists")]
    RenameConflict,
    /// The replacement produces an invalid file name.
    #[error("the replacement produces an invalid file name")]
    InvalidName,
    /// Open documents cannot be renamed.
    #[error("an open document cannot be renamed")]
    SnapshotRename,
    /// Running the regex/replacement failed.
    #[error("the replacement failed: {0}")]
    Match(String),
}

/// One skipped file/match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceIssue {
    /// The file, when known.
    pub file: Option<FileId>,
    /// Its path.
    pub path: Option<PathBuf>,
    /// Why it was skipped.
    pub kind: ReplaceIssueKind,
}

/// A file that was (or would be) changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReplaceOutcome {
    /// The file.
    pub file: FileId,
    /// Its path (before any rename).
    pub path: PathBuf,
    /// Whether it was an open-document snapshot.
    pub snapshot: bool,
    /// Number of matches whose replacement was computed.
    pub replaced: usize,
    /// Content edits in the original decoded text (byte offsets); the UI can
    /// apply these to its own buffer when `snapshot` is true.
    pub edits: Vec<TextEdit>,
    /// The file was written to disk.
    pub written: bool,
    /// The new path when the file name changes.
    pub renamed_to: Option<PathBuf>,
}

/// What happened to a rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameStatus {
    /// The file was renamed.
    Renamed,
    /// The plan was a dry run.
    DryRun,
    /// The rename was skipped (conflict or policy).
    Skipped,
    /// The rename failed.
    Failed,
}

/// One planned or performed rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameOutcome {
    /// The file.
    pub file: FileId,
    /// Old path.
    pub from: PathBuf,
    /// New path.
    pub to: PathBuf,
    /// Result.
    pub status: RenameStatus,
}

/// The result of [`ReplacePlan::execute`].
#[derive(Debug, Clone, Default)]
pub struct ReplaceReport {
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Files that changed (or would change).
    pub files: Vec<FileReplaceOutcome>,
    /// Renames.
    pub renames: Vec<RenameOutcome>,
    /// Skipped files/matches and errors.
    pub issues: Vec<ReplaceIssue>,
    /// The undo journal (`None` for dry runs).
    pub journal: Option<ReplaceJournal>,
    /// Cancellation stopped the run.
    pub cancelled: bool,
}

impl ReplaceReport {
    /// Number of files that changed.
    pub fn changed_files(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.written || f.snapshot)
            .count()
    }

    /// Number of matches replaced.
    pub fn replaced_matches(&self) -> usize {
        self.files.iter().map(|f| f.replaced).sum()
    }

    /// Whether anything changed.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.renames.is_empty()
    }
}

/// One reversible step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEntry {
    /// Restore a pre-existing rename target after moving the source back.
    RestoreReplacedFile {
        /// The original target path.
        path: PathBuf,
        /// The original file, moved aside in the same directory. Keeping the
        /// original inode preserves its contents and filesystem metadata.
        backup: PathBuf,
    },
    /// A content write; `before` restores the original bytes.
    Write {
        /// The file.
        path: PathBuf,
        /// The original bytes (BOM included).
        before: Vec<u8>,
        /// Hash of the bytes after the replace (detects later edits).
        after_hash: u64,
        /// The file's encoding.
        encoding: TextEncoding,
    },
    /// A rename (`from` -> `to`).
    Rename {
        /// Old path.
        from: PathBuf,
        /// New path.
        to: PathBuf,
    },
}

/// Undo information for one project replace ("Undo Replace").
#[derive(Debug, Clone, Default)]
pub struct ReplaceJournal {
    entries: Vec<JournalEntry>,
}

impl ReplaceJournal {
    fn new() -> Self {
        Self::default()
    }

    /// The steps, in execution order.
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    fn push(&mut self, entry: JournalEntry) {
        self.entries.push(entry);
    }

    /// Nothing to undo.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of journal steps.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The menu title: "Undo Replace".
    pub fn label(&self) -> &'static str {
        "Undo Replace"
    }

    /// Reverts steps in reverse order. If a step fails, the remaining journal
    /// is returned in [`UndoReport::remaining`] and may be retried.
    pub fn undo(self) -> UndoReport {
        self.undo_with(&CancelToken::new())
    }

    /// [`undo`](Self::undo) with cancellation between steps.
    pub fn undo_with(mut self, cancel: &CancelToken) -> UndoReport {
        let mut report = UndoReport::default();
        while let Some(entry) = self.entries.last() {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            let result = match entry {
                JournalEntry::Rename { from, to } => {
                    // On a case-insensitive volume the two spellings can name
                    // the same file. Never use that exception for symlinks.
                    let same = same_nonsymlink_path(from, to);
                    let moved = if same {
                        fs::rename(to, from)
                    } else {
                        rename_exclusive(to, from)
                    };
                    moved
                        .map(|()| report.renamed.push((to.clone(), from.clone())))
                        .map_err(|err| UndoFailure {
                            path: to.clone(),
                            reason: err.to_string(),
                        })
                }
                JournalEntry::RestoreReplacedFile { path, backup } => {
                    rename_exclusive(backup, path)
                        .map(|()| report.restored.push(path.clone()))
                        .map_err(|err| UndoFailure {
                            path: path.clone(),
                            reason: format!("{err}; original is retained at {}", backup.display()),
                        })
                }
                JournalEntry::Write {
                    path,
                    before,
                    after_hash,
                    ..
                } => fs::read(path)
                    .and_then(|bytes| {
                        if content_hash(&bytes) != *after_hash {
                            return Err(io::Error::other("the file changed after the replace"));
                        }
                        write_atomic(path, before, *after_hash)
                    })
                    .map(|()| report.restored.push(path.clone()))
                    .map_err(|err| UndoFailure {
                        path: path.clone(),
                        reason: err.to_string(),
                    }),
            };
            match result {
                Ok(()) => {
                    self.entries.pop();
                }
                Err(failure) => {
                    // Earlier entries can depend on this one (e.g. a write
                    // followed by a rename), so do not undo past a failure.
                    report.failures.push(failure);
                    break;
                }
            }
        }
        report.remaining = self;
        report
    }
}

/// Result of [`ReplaceJournal::undo`].
#[derive(Debug, Clone, Default)]
pub struct UndoReport {
    /// Files whose contents were restored.
    pub restored: Vec<PathBuf>,
    /// Renames reverted (`new` -> `old`).
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// Steps that could not be undone.
    pub failures: Vec<UndoFailure>,
    /// Cancellation stopped the undo.
    pub cancelled: bool,
    /// Steps still to undo after a failure or cancellation. Retry this journal
    /// after resolving the conflict; it is empty after a complete undo.
    pub remaining: ReplaceJournal,
}

/// One step that could not be undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoFailure {
    /// The path involved.
    pub path: PathBuf,
    /// Why it was not restored.
    pub reason: String,
}

struct PendingRename {
    file: FileId,
    from: PathBuf,
    to: PathBuf,
}

/// Replaces a regular file through an exclusively-created temporary file.
/// Reject symlinks and recheck the file's identity and contents before the
/// atomic rename, so a stale search does not knowingly overwrite newer work.
fn write_atomic(path: &Path, bytes: &[u8], expected_hash: u64) -> io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let original = fs::symlink_metadata(path)?;
    if !original.is_file() {
        return Err(io::Error::other("replacement target is not a regular file"));
    }
    let source = fs::File::open(path)?;
    if !same_file_identity(&original, &source.metadata()?) {
        return Err(io::Error::other("the file changed during replacement"));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (tmp, mut file) = (0..128)
        .find_map(|_| {
            let candidate = parent.join(format!(
                ".scratch-replace-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => Some(Ok((candidate, file))),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => None,
                Err(err) => Some(Err(err)),
            }
        })
        .unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "no free temporary name",
            ))
        })?;
    let result = (|| {
        file.write_all(bytes)?;
        copy_file_attributes(&source, &file)?;
        file.set_permissions(original.permissions())?;
        file.sync_all()?;
        let current = fs::symlink_metadata(path)?;
        if !current.is_file()
            || !same_file_identity(&original, &current)
            || content_hash(&fs::read(path)?) != expected_hash
        {
            return Err(io::Error::other("the file changed during replacement"));
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn same_file_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

#[cfg(unix)]
fn copy_file_attributes(source: &fs::File, target: &fs::File) -> io::Result<()> {
    use xattr::FileExt;
    let names = match source.list_xattr() {
        Ok(names) => names,
        Err(err) if err.kind() == io::ErrorKind::Unsupported => return Ok(()),
        Err(err) => return Err(err),
    };
    for name in names {
        let value = source
            .get_xattr(&name)?
            .ok_or_else(|| io::Error::other("an extended attribute changed during replacement"))?;
        target.set_xattr(&name, &value)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn copy_file_attributes(_source: &fs::File, _target: &fs::File) -> io::Result<()> {
    Ok(())
}

/// Renames without deleting an unjournaled destination. A replaced file is
/// moved aside first, so its original inode and metadata survive undo.
fn rename_path(
    from: &Path,
    to: &Path,
    policy: ConflictPolicy,
    journal: &mut ReplaceJournal,
) -> Result<bool, String> {
    if from == to {
        return Ok(false);
    }
    // Another process may create the destination between inspection and the
    // exclusive rename. Re-evaluate the chosen conflict policy in that case.
    for _ in 0..16 {
        match fs::symlink_metadata(to) {
            Ok(metadata) => {
                if same_nonsymlink_path(from, to) {
                    fs::rename(from, to).map_err(|e| e.to_string())?;
                    journal.push(JournalEntry::Rename {
                        from: from.to_owned(),
                        to: to.to_owned(),
                    });
                    return Ok(true);
                }
                match policy {
                    ConflictPolicy::Skip => return Ok(false),
                    ConflictPolicy::Fail => {
                        return Err("a file with the new name already exists".into());
                    }
                    ConflictPolicy::Overwrite => {}
                }
                if !metadata.is_file() {
                    return Err("the rename target is not a regular file".into());
                }
                if fs::symlink_metadata(from)
                    .map_err(|e| e.to_string())?
                    .is_dir()
                {
                    return Err("a folder cannot replace a file".into());
                }
                let backup = move_to_backup(to).map_err(|e| e.to_string())?;
                journal.push(JournalEntry::RestoreReplacedFile {
                    path: to.to_owned(),
                    backup: backup.clone(),
                });
                // The target could have changed just before it was moved.
                let result = fs::symlink_metadata(&backup).and_then(|m| {
                    if m.is_file() {
                        rename_exclusive(from, to)
                    } else {
                        Err(io::Error::other("the rename target changed type"))
                    }
                });
                match result {
                    Ok(()) => {
                        journal.push(JournalEntry::Rename {
                            from: from.to_owned(),
                            to: to.to_owned(),
                        });
                        return Ok(true);
                    }
                    Err(err) => {
                        if rename_exclusive(&backup, to).is_ok() {
                            journal.entries.pop();
                            return Err(err.to_string());
                        }
                        // Retain this step so it can be retried even if a new
                        // file appeared at the target during the operation.
                        return Err(format!(
                            "{err}; original target retained at {}",
                            backup.display()
                        ));
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => match rename_exclusive(from, to) {
                Ok(()) => {
                    journal.push(JournalEntry::Rename {
                        from: from.to_owned(),
                        to: to.to_owned(),
                    });
                    return Ok(true);
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.to_string()),
            },
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("the rename target changed repeatedly".into())
}

fn same_nonsymlink_path(a: &Path, b: &Path) -> bool {
    let regular_paths = fs::symlink_metadata(a)
        .ok()
        .zip(fs::symlink_metadata(b).ok())
        .is_some_and(|(a, b)| !a.file_type().is_symlink() && !b.file_type().is_symlink());
    regular_paths
        && fs::canonicalize(a)
            .ok()
            .zip(fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}

fn move_to_backup(path: &Path) -> io::Result<PathBuf> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or(Path::new("."));
    for _ in 0..128 {
        let backup = parent.join(format!(
            ".scratch-replace-backup-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        match rename_exclusive(path, &backup) {
            Ok(()) => return Ok(backup),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique backup name",
    ))
}

/// A check followed by `fs::rename` is not sufficient: another process can
/// create the target between the two operations. Refuse unsupported platforms
/// rather than silently falling back to an overwriting rename.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in source path"))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in target path"))?;
    // SAFETY: both C strings are NUL-terminated and live for the syscall.
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    // SAFETY: both C strings are NUL-terminated and live for the syscall.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rename_exclusive(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "non-overwriting rename is not supported on this platform",
    ))
}
