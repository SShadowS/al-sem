//! **Every changed line accounted** (agent-tasks design §8.3, astra a10): for
//! each file of a diff, every changed line (added on the new side, removed
//! from the old) lands in exactly one bucket:
//!
//! - `routine-body` / `routine-signature`, with the routine: the model's
//!   routine span index (the one that resolves the changed roots,
//!   [`build_routine_span_index`]), split at the body's first `var` or
//!   `begin` line ([`body_start`]): the attributes and header above it are
//!   the signature;
//! - `object-declaration` by kind ([`DeclKind`]): the line scanner
//!   ([`super::scan`]) over the analyzed text;
//! - `outside-object`: before, between or after objects;
//! - `removed-routine-text`: removed lines that held a routine no new-side
//!   routine span holds (a routine removed or moved). Without a base they
//!   are not called deleted (§8.2: deleted needs complete inventories), and
//!   the header names found in them are listed;
//! - `routine-not-in-model`: routine text the model holds no routine span for;
//! - `unmapped`, with the reason: a path that names no workspace file (or
//!   two), a non-AL file, a deleted file (no base), a file whose hunk header
//!   the parser rejected, a line past the end of the analyzed file.
//!
//! Invariant ([`Buckets::total`]): per file, the buckets' line counts sum to
//! the file's added plus removed lines, and a hunk that touches a global and
//! a routine reports both. Each hunk keeps its own buckets, so a hunk that
//! touches no routine (digest's `hunk-outside-routines`) is named with what
//! it touched.
//!
//! The diff is also checked against the analyzed text (§8.2): every new-side
//! line a hunk shows (context and added) must be the analyzed file's line at
//! that number. A file that fails is reported with its first differing line;
//! its routines' positions are uncertain.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Value, json};

use super::scan::{DeclKind, LineKind, Scanner, body_start};
use crate::engine::l5::diff_parser::{DiffFileKind, DiffParseResult, HunkChange};
use crate::engine::l5::digest_cli::{DiffRoot, RoutineSpanIndex, file_key, is_al_path, path_keys};
use crate::program::inputs::WsFile;

/// Why a file's lines map to no declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Unmapped {
    /// The path names no workspace `.al` file.
    NotInWorkspace,
    /// The path names two workspace files (as written and below the git top).
    Ambiguous,
    /// Not an AL file.
    NotAl,
    /// A deleted file: its lines need a base to be placed.
    DeletedFile,
    /// The parser rejected a hunk header of the file: its changes are unknown.
    DiffParseError,
    /// A changed line past the end of the analyzed file.
    PastEndOfFile,
}

impl Unmapped {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Unmapped::NotInWorkspace => "not-in-workspace",
            Unmapped::Ambiguous => "ambiguous-path",
            Unmapped::NotAl => "not-al",
            Unmapped::DeletedFile => "deleted-file",
            Unmapped::DiffParseError => "diff-parse-error",
            Unmapped::PastEndOfFile => "past-end-of-file",
        }
    }

    #[must_use]
    pub fn human(self) -> &'static str {
        match self {
            Unmapped::NotInWorkspace => "not in the workspace",
            Unmapped::Ambiguous => "path names two workspace files",
            Unmapped::NotAl => "not an AL file",
            Unmapped::DeletedFile => "deleted file, needs a base",
            Unmapped::DiffParseError => "hunk header rejected",
            Unmapped::PastEndOfFile => "past the end of the analyzed file",
        }
    }
}

/// Line counts per bucket.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buckets {
    pub body: usize,
    pub signature: usize,
    pub decl: BTreeMap<DeclKind, usize>,
    pub outside: usize,
    pub removed_routine_text: usize,
    pub routine_not_in_model: usize,
    pub unmapped: BTreeMap<Unmapped, usize>,
}

impl Buckets {
    /// Every line counted, once.
    #[must_use]
    pub fn total(&self) -> usize {
        self.body
            + self.signature
            + self.declarations()
            + self.outside
            + self.removed_routine_text
            + self.routine_not_in_model
            + self.unmapped.values().sum::<usize>()
    }

    /// The object-declaration lines.
    #[must_use]
    pub fn declarations(&self) -> usize {
        self.decl.values().sum()
    }

    fn add(&mut self, o: &Buckets) {
        self.body += o.body;
        self.signature += o.signature;
        for (k, n) in &o.decl {
            *self.decl.entry(*k).or_default() += n;
        }
        self.outside += o.outside;
        self.removed_routine_text += o.removed_routine_text;
        self.routine_not_in_model += o.routine_not_in_model;
        for (k, n) in &o.unmapped {
            *self.unmapped.entry(*k).or_default() += n;
        }
    }

    fn put(&mut self, b: Bucket, n: usize) {
        match b {
            Bucket::Body => self.body += n,
            Bucket::Signature => self.signature += n,
            Bucket::Decl(k) => *self.decl.entry(k).or_default() += n,
            Bucket::Outside => self.outside += n,
            Bucket::RemovedRoutineText => self.removed_routine_text += n,
            Bucket::RoutineNotInModel => self.routine_not_in_model += n,
            Bucket::Unmapped(u) => *self.unmapped.entry(u).or_default() += n,
        }
    }

    /// The JSON form (every bucket, zeros included, so a reader can sum).
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "total": self.total(),
            "routineBody": self.body,
            "routineSignature": self.signature,
            "objectDeclaration": {
                "total": self.declarations(),
                "byKind": DeclKind::ALL.iter().map(|k| (k.as_str().to_string(), json!(self.decl.get(k).copied().unwrap_or(0)))).collect::<serde_json::Map<_, _>>(),
            },
            "outsideObject": self.outside,
            "removedRoutineText": self.removed_routine_text,
            "routineNotInModel": self.routine_not_in_model,
            "unmapped": {
                "total": self.unmapped.values().sum::<usize>(),
                "byReason": self.unmapped.iter().map(|(k, n)| (k.as_str().to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
            },
        })
    }
}

/// One line's bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bucket {
    Body,
    Signature,
    Decl(DeclKind),
    Outside,
    RemovedRoutineText,
    RoutineNotInModel,
    Unmapped(Unmapped),
}

/// One hunk's account.
#[derive(Clone, Debug, Default)]
pub struct HunkAccount {
    /// 1-based new-side range, as digest's `hunk-outside-routines` names it:
    /// `new_start ..= new_start + max(new_count - 1, 0)`.
    pub start: i32,
    pub end: i32,
    pub buckets: Buckets,
    /// The routines (stable ids) the hunk's changed lines are in.
    pub routines: BTreeSet<String>,
}

/// The analyzed file disagrees with the diff's new side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// The first new-side line (1-based) whose text differs.
    pub line: i32,
    /// How many shown new-side lines differ.
    pub lines: usize,
}

/// One diff file's account.
#[derive(Clone, Debug)]
pub struct FileAccount {
    /// The diff's path (its new path; a deletion's old one).
    pub path: String,
    pub old_path: Option<String>,
    pub kind: &'static str,
    /// The workspace file it names (`rel_posix`), when one.
    pub workspace_file: Option<String>,
    pub added: usize,
    pub removed: usize,
    pub buckets: Buckets,
    pub hunks: Vec<HunkAccount>,
    /// Routine names found in removed routine text (header lines).
    pub removed_routines: BTreeSet<String>,
    /// Why the whole file is unmapped.
    pub unmapped: Option<Unmapped>,
    pub mismatch: Option<Mismatch>,
}

/// Per routine (stable id): its changed lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoutineLines {
    pub body: usize,
    pub signature: usize,
    /// Of `body + signature`, the `+` lines (the rest are `-` lines).
    pub added: usize,
    /// The routine's header line (its declaration's anchor line) is a `+`
    /// line of the diff.
    pub header_added: bool,
    /// A run of `-` lines sits inside the routine's span or right at its
    /// edges (just before its first line or just after its last), whichever
    /// bucket the run was counted in.
    pub removed_near: bool,
}

impl RoutineLines {
    /// The diff introduces this declaration (output validation F25): its
    /// header line is a `+` line and no `-` line sits in or at the edges of
    /// its span. Then the old text had no header at this place (a rewritten
    /// header shows a `-` line, and a span holds one header), even when the
    /// diff aligns the routine's last lines (`end;`) with the old text as
    /// context (c20). What a diff alone can prove; only a base can say the
    /// routine is new to the program.
    #[must_use]
    pub fn added_in_diff(&self) -> bool {
        self.header_added && !self.removed_near && self.added == self.body + self.signature
    }
}

/// The whole diff's account.
#[derive(Clone, Debug, Default)]
pub struct Accounting {
    pub files: Vec<FileAccount>,
    pub totals: Buckets,
    pub routines: BTreeMap<String, RoutineLines>,
    /// `diff -r`'s `Only in` paths (no hunk, so no line), with the workspace
    /// file each names, when one.
    pub only_in: Vec<(String, Option<String>)>,
    /// Parse errors that name no file (a truncated hunk).
    pub errors: Vec<String>,
}

impl Accounting {
    /// Changed lines, added plus removed.
    #[must_use]
    pub fn changed_lines(&self) -> usize {
        self.files.iter().map(|f| f.added + f.removed).sum()
    }

    /// The hunks that touch no routine, with their file.
    pub fn hunks_outside_routines(&self) -> impl Iterator<Item = (&FileAccount, &HunkAccount)> {
        self.files.iter().flat_map(|f| {
            f.hunks
                .iter()
                .filter(|h| h.routines.is_empty())
                .map(move |h| (f, h))
        })
    }

    /// The workspace files whose analyzed text disagrees with the diff.
    pub fn mismatched(&self) -> impl Iterator<Item = &FileAccount> {
        self.files.iter().filter(|f| f.mismatch.is_some())
    }
}

/// Account for `diff` against the workspace's analyzed `files` and the
/// model's `spans`, reading paths under `root` as the changed roots are read.
#[must_use]
pub fn account(
    diff: &DiffParseResult,
    root: &DiffRoot,
    files: &[WsFile],
    spans: &RoutineSpanIndex,
) -> Accounting {
    let mut by_key: HashMap<String, Vec<&WsFile>> = HashMap::new();
    for f in files {
        by_key.entry(file_key(&f.rel_posix)).or_default().push(f);
    }
    let resolve = |path: &str| -> Result<&WsFile, Unmapped> {
        let hits: Vec<&String> = path_keys(root, path)
            .iter()
            .filter_map(|k| by_key.get_key_value(k).map(|(k, _)| k))
            .collect();
        match hits.as_slice() {
            [k] => {
                let fs = &by_key[*k];
                // A key holding two files (case only) is ambiguous too.
                if fs.len() == 1 {
                    Ok(fs[0])
                } else {
                    Err(Unmapped::Ambiguous)
                }
            }
            [] => Err(Unmapped::NotInWorkspace),
            _ => Err(Unmapped::Ambiguous),
        }
    };
    let poisoned: BTreeSet<&str> = diff.file_errors.iter().map(|e| e.path.as_str()).collect();
    let mut out = Accounting {
        errors: diff.errors.clone(),
        ..Accounting::default()
    };
    for df in &diff.files {
        let kind = match df.kind {
            DiffFileKind::Modified => "modified",
            DiffFileKind::Added => "added",
            DiffFileKind::Deleted => "deleted",
            DiffFileKind::Renamed => "renamed",
        };
        let added: usize = df
            .hunks
            .iter()
            .map(|h| {
                h.changes
                    .iter()
                    .filter(|c| matches!(c, HunkChange::Added(_)))
                    .count()
            })
            .sum();
        let removed: usize = df.hunks.iter().flat_map(|h| &h.deleted).map(Vec::len).sum();
        let mut fa = FileAccount {
            path: df.path.clone(),
            old_path: df.old_path.clone(),
            kind,
            workspace_file: None,
            added,
            removed,
            buckets: Buckets::default(),
            hunks: Vec::new(),
            removed_routines: BTreeSet::new(),
            unmapped: None,
            mismatch: None,
        };
        let whole = if df.kind == DiffFileKind::Deleted {
            Some(if is_al_path(&df.path) || df.old_path.is_none() {
                Unmapped::DeletedFile
            } else {
                Unmapped::NotAl
            })
        } else if !is_al_path(&df.path) {
            Some(Unmapped::NotAl)
        } else if poisoned.contains(df.path.as_str())
            || df.old_path.as_deref().is_some_and(|p| poisoned.contains(p))
        {
            Some(Unmapped::DiffParseError)
        } else {
            None
        };
        let ws = match whole {
            Some(u) => Err(u),
            None => resolve(&df.path),
        };
        match ws {
            Err(u) => {
                fa.unmapped = Some(u);
                for h in &df.hunks {
                    let mut ha = hunk_account(h);
                    let n = h
                        .changes
                        .iter()
                        .filter(|c| matches!(c, HunkChange::Added(_)))
                        .count()
                        + h.deleted.iter().map(Vec::len).sum::<usize>();
                    ha.buckets.put(Bucket::Unmapped(u), n);
                    fa.buckets.add(&ha.buckets);
                    fa.hunks.push(ha);
                }
            }
            Ok(file) => {
                fa.workspace_file = Some(file.rel_posix.clone());
                account_file(df, file, spans, &mut fa, &mut out.routines);
            }
        }
        debug_assert_eq!(fa.buckets.total(), fa.added + fa.removed, "{}", fa.path);
        out.totals.add(&fa.buckets);
        out.files.push(fa);
    }
    for p in &diff.only_in {
        out.only_in
            .push((p.clone(), resolve(p).ok().map(|f| f.rel_posix.clone())));
    }
    out
}

fn hunk_account(h: &crate::engine::l5::diff_parser::DiffHunk) -> HunkAccount {
    HunkAccount {
        start: h.new_start,
        end: h.new_start + (h.new_count - 1).max(0),
        ..HunkAccount::default()
    }
}

/// One mapped file: its routine spans, its scanned lines, every change.
fn account_file(
    df: &crate::engine::l5::diff_parser::DiffFile,
    file: &WsFile,
    spans: &RoutineSpanIndex,
    fa: &mut FileAccount,
    routines: &mut BTreeMap<String, RoutineLines>,
) {
    let text: &str = file.text.as_deref().unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    // This file's spans: the unit is `ws:<rel_posix>`.
    let unit = format!("ws:{}", file.rel_posix);
    let mine: Vec<usize> = (0..spans.spans.len())
        .filter(|&i| spans.spans[i].source_unit_id == unit)
        .collect();
    // The span holding each line (0-based), and each span's body start.
    let mut owner: Vec<Option<usize>> = vec![None; lines.len() + 1];
    for &i in &mine {
        let s = &spans.spans[i];
        let last = (s.end as usize).min(lines.len());
        for o in owner.iter_mut().take(last + 1).skip(s.start as usize) {
            *o = Some(i);
        }
    }
    let mut body_of: HashMap<usize, Option<usize>> = HashMap::new();
    let mut body = |i: usize| -> Option<usize> {
        *body_of.entry(i).or_insert_with(|| {
            let s = &spans.spans[i];
            body_start(&lines, s.anchor_line as usize, s.end as usize)
        })
    };
    // The scanner's labels, and its state after each line a deletion follows.
    let gaps: BTreeSet<usize> = df
        .hunks
        .iter()
        .flat_map(|h| &h.changes)
        .filter_map(|c| match *c {
            HunkChange::DeletedAfter(l) | HunkChange::DeletedAttributesAfter(l) => {
                Some(l.max(0) as usize)
            }
            HunkChange::Added(_) => None,
        })
        .collect();
    let mut states: HashMap<usize, Scanner> = HashMap::new();
    if gaps.contains(&0) {
        states.insert(0, Scanner::new());
    }
    let labels = Scanner::new().scan(lines.iter().copied(), |i, s| {
        if gaps.contains(&(i + 1)) {
            states.insert(i + 1, s.clone());
        }
    });
    let mut mismatched = Vec::new();
    // F25: the spans a `-` run sits in or at the edges of.
    let mut removed_near: BTreeSet<usize> = BTreeSet::new();
    for h in &df.hunks {
        for c in &h.changes {
            if let HunkChange::DeletedAfter(l) | HunkChange::DeletedAttributesAfter(l) = *c {
                let next0 = l.max(0) as usize; // line l + 1, 0-based
                removed_near.extend(mine.iter().copied().filter(|&i| {
                    let s = &spans.spans[i];
                    (s.start as usize) <= next0 && next0 <= s.end as usize + 1
                }));
            }
        }
    }
    for h in &df.hunks {
        let mut ha = hunk_account(h);
        // The diff against the analyzed text.
        for (n, t) in &h.new_side {
            let ok = (*n as usize)
                .checked_sub(1)
                .and_then(|i| lines.get(i))
                // A byte-order mark is not text: the analyzed text has it
                // stripped, the diff keeps it (a commit that adds one).
                .is_some_and(|l| {
                    l.trim_end_matches('\r').trim_start_matches('\u{feff}')
                        == t.trim_start_matches('\u{feff}')
                });
            if !ok {
                mismatched.push(*n);
            }
        }
        let mut runs = h.deleted.iter();
        for c in &h.changes {
            match *c {
                HunkChange::Added(n) => {
                    let ix = (n as usize).wrapping_sub(1);
                    let b = if n < 1 || ix >= lines.len() {
                        Bucket::Unmapped(Unmapped::PastEndOfFile)
                    } else if let Some(i) = owner[ix] {
                        let rid = &spans.spans[i].routine;
                        ha.routines.insert(rid.clone());
                        let e = routines.entry(rid.clone()).or_default();
                        e.added += 1;
                        if ix == spans.spans[i].anchor_line as usize {
                            e.header_added = true;
                        }
                        if body(i).is_some_and(|b| ix >= b) {
                            e.body += 1;
                            Bucket::Body
                        } else {
                            e.signature += 1;
                            Bucket::Signature
                        }
                    } else {
                        match &labels[ix] {
                            LineKind::Outside => Bucket::Outside,
                            LineKind::Decl(k) => Bucket::Decl(*k),
                            LineKind::Routine { .. } => Bucket::RoutineNotInModel,
                        }
                    };
                    ha.buckets.put(b, 1);
                }
                HunkChange::DeletedAfter(l) | HunkChange::DeletedAttributesAfter(l) => {
                    let run: &[String] = runs.next().map_or(&[], Vec::as_slice);
                    let touching = mine.iter().copied().find(|&i| spans.spans[i].touched_by(c));
                    if let Some(i) = touching {
                        let rid = &spans.spans[i].routine;
                        ha.routines.insert(rid.clone());
                        let e = routines.entry(rid.clone()).or_default();
                        // Deleted lines sit between new lines l and l + 1: in
                        // the signature when line l + 1 comes before the body.
                        let next0 = l.max(0) as usize; // line l + 1, 0-based
                        if body(i).is_some_and(|b| next0 >= b) {
                            e.body += run.len();
                            ha.buckets.put(Bucket::Body, run.len());
                        } else {
                            e.signature += run.len();
                            ha.buckets.put(Bucket::Signature, run.len());
                        }
                        continue;
                    }
                    let mut s = states
                        .get(&(l.max(0) as usize))
                        .cloned()
                        .unwrap_or_default();
                    for k in s.scan(run.iter().map(String::as_str), |_, _| {}) {
                        let b = match k {
                            LineKind::Outside => Bucket::Outside,
                            LineKind::Decl(k) => Bucket::Decl(k),
                            LineKind::Routine { name } => {
                                if let Some(n) = name {
                                    fa.removed_routines.insert(n);
                                }
                                Bucket::RemovedRoutineText
                            }
                        };
                        ha.buckets.put(b, 1);
                    }
                }
            }
        }
        fa.buckets.add(&ha.buckets);
        fa.hunks.push(ha);
    }
    for i in removed_near {
        if let Some(e) = routines.get_mut(&spans.spans[i].routine) {
            e.removed_near = true;
        }
    }
    if let Some(&first) = mismatched.first() {
        fa.mismatch = Some(Mismatch {
            line: first,
            lines: mismatched.len(),
        });
    }
}

#[cfg(test)]
#[path = "accounting_tests.rs"]
mod tests;
