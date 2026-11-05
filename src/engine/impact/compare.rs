//! **The comparison tuple** (agent-tasks design §8.2, astra a10): what an
//! impact answer compares, resolved ONCE, printed, and read by everything
//! after it (root resolution, accounting; B2's base build and delta).
//!
//! | Field | Meaning |
//! |---|---|
//! | `requestedRef` | what the caller named (`origin/master`, a commit), or none |
//! | `resolvedCommit` | the commit it names (`git rev-parse <ref>^{commit}`) |
//! | `mergeBase` | `git merge-base resolvedCommit HEAD` |
//! | `comparisonBase` | the commit the change is measured against: `mergeBase` when `--base` is given; none for a diff alone (B1 builds no base, so a diff's pre-images are not checked against one) |
//! | `head` | `HEAD`'s commit and the analyzed revision's `inputsStamp` (a resident engine's manifest stamp; none for a one-shot run) |
//! | `diffHash` | blake3 of the diff text the answer used |
//!
//! `--base REV` alone makes the change `comparisonBase` → working tree:
//! committed (merge base to `HEAD`), staged and unstaged changes, plus every
//! untracked `.al` file under the workspace (listed, and read as a new file,
//! so its routines resolve). Each git command runs with the workspace as its
//! directory and reads only; nothing in the repository is changed.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

/// The tuple (see the module doc).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    /// How the change was given: `base`, `diff`, `files` or `routines`.
    pub source: &'static str,
    pub requested_ref: Option<String>,
    pub resolved_commit: Option<String>,
    pub merge_base: Option<String>,
    pub comparison_base: Option<String>,
    /// Why `comparison_base` is what it is.
    pub comparison_base_note: String,
    pub head_commit: Option<String>,
    pub inputs_stamp: Option<String>,
    pub diff_hash: Option<String>,
    /// `--base`: the workspace files changed in each layer, top-relative.
    pub committed: Vec<String>,
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    /// Untracked `.al` files under the workspace, top-relative.
    pub untracked: Vec<String>,
}

impl Comparison {
    /// The JSON form (`al-sem.impact/1`'s `comparison`).
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "source": self.source,
            "requestedRef": self.requested_ref,
            "resolvedCommit": self.resolved_commit,
            "mergeBase": self.merge_base,
            "comparisonBase": self.comparison_base,
            "comparisonBaseNote": self.comparison_base_note,
            "head": {"commit": self.head_commit, "inputsStamp": self.inputs_stamp},
            "diffHash": self.diff_hash,
            "layers": {
                "committed": self.committed,
                "staged": self.staged,
                "unstaged": self.unstaged,
                "untracked": self.untracked,
            },
        })
    }

    /// The tuple read back from [`Comparison::to_json`] (a part's answer).
    #[must_use]
    pub fn from_json(v: &Value) -> Comparison {
        let s = |k: &str| v[k].as_str().map(str::to_string);
        let list = |k: &str| -> Vec<String> {
            v["layers"][k]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        };
        Comparison {
            source: match v["source"].as_str() {
                Some("base") => "base",
                Some("base+diff") => "base+diff",
                Some("diff") => "diff",
                Some("files") => "files",
                _ => "routines",
            },
            requested_ref: s("requestedRef"),
            resolved_commit: s("resolvedCommit"),
            merge_base: s("mergeBase"),
            comparison_base: s("comparisonBase"),
            comparison_base_note: s("comparisonBaseNote").unwrap_or_default(),
            head_commit: v["head"]["commit"].as_str().map(str::to_string),
            inputs_stamp: v["head"]["inputsStamp"].as_str().map(str::to_string),
            diff_hash: s("diffHash"),
            committed: list("committed"),
            staged: list("staged"),
            unstaged: list("unstaged"),
            untracked: list("untracked"),
        }
    }
}

/// blake3 of a diff's text, hex.
#[must_use]
pub fn diff_hash(text: &str) -> String {
    format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
}

/// `git <args>` in `dir`: its stdout (trimmed of the final newline), or why
/// it failed.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if s.ends_with('\n') {
        s.pop();
    }
    Ok(s)
}

fn lines(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `HEAD`'s commit in `workspace`'s repository, when it is one.
#[must_use]
pub fn head_commit(workspace: &Path) -> Option<String> {
    git(workspace, &["rev-parse", "--verify", "-q", "HEAD"]).ok()
}

/// The tuple of a change given as a diff text (or files, or routines): no
/// base is resolved.
#[must_use]
pub fn for_input(source: &'static str, workspace: &Path, diff: Option<&str>) -> Comparison {
    Comparison {
        source,
        comparison_base_note: match source {
            "diff" => "none: a diff alone names no base (no before/after; B2 checks the diff's pre-images against a base)".into(),
            _ => "none: no diff and no base were given (no before/after)".into(),
        },
        head_commit: head_commit(workspace),
        diff_hash: diff.map(diff_hash),
        ..Comparison::default()
    }
}

/// `--base REV`: the tuple and the change's diff text (merge base to the
/// working tree, the workspace's paths only, plus every untracked `.al`
/// file as a new file), paths relative to the git top level.
pub fn for_base(workspace: &Path, rev: &str) -> Result<(Comparison, String), String> {
    let resolved = git(
        workspace,
        &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
    )
    .map_err(|_| format!("impact: --base '{rev}' names no commit"))?;
    let head = git(workspace, &["rev-parse", "--verify", "-q", "HEAD"])
        .map_err(|e| format!("impact: the workspace has no HEAD commit: {e}"))?;
    let base = git(workspace, &["merge-base", &resolved, &head])
        .map_err(|e| format!("impact: no merge base of '{rev}' and HEAD: {e}"))?; // The workspace's own paths, top-relative (`git diff` prints those).
    let names = |args: &[&str]| -> Result<Vec<String>, String> {
        let mut a = args.to_vec();
        a.extend(["--name-only", "--", "."]);
        git(workspace, &a).map(|s| lines(&s))
    };
    let committed = names(&["diff", &base, &head])?;
    let staged = names(&["diff", "--cached"])?;
    let unstaged = names(&["diff"])?;
    let untracked: Vec<String> = lines(&git(
        workspace,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--full-name",
            "--",
            ".",
        ],
    )?)
    .into_iter()
    .filter(|p| crate::engine::l5::digest_cli::is_al_path(p))
    .collect();
    let mut text = git(
        workspace,
        // The user's config must not change the text the parser reads
        // (`diff.mnemonicPrefix` writes `c/` and `w/`, `diff.noprefix` none).
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &base,
            "--",
            ".",
        ],
    )?;
    if !text.is_empty() {
        text.push('\n');
    }
    let top = git(workspace, &["rev-parse", "--show-toplevel"])?;
    for p in &untracked {
        let body = std::fs::read_to_string(Path::new(&top).join(p))
            .map_err(|e| format!("impact: untracked file '{p}' could not be read: {e}"))?;
        text.push_str(&new_file_section(p, &body));
    }
    let cmp = Comparison {
        source: "base",
        requested_ref: Some(rev.to_string()),
        resolved_commit: Some(resolved),
        merge_base: Some(base.clone()),
        comparison_base: Some(base),
        comparison_base_note: "the merge base of the requested ref and HEAD".into(),
        head_commit: Some(head),
        inputs_stamp: None,
        diff_hash: Some(diff_hash(&text)),
        committed,
        staged,
        unstaged,
        untracked,
    };
    Ok((cmp, text))
}

/// `--base REV --diff P` (B2): the diff text is the change; the comparison
/// base is the merge base of REV and HEAD, against whose text the diff's
/// pre-images are checked (by the base part's accounting) before any delta.
pub fn for_base_and_diff(workspace: &Path, rev: &str, diff: &str) -> Result<Comparison, String> {
    let resolved = git(
        workspace,
        &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
    )
    .map_err(|_| format!("impact: --base '{rev}' names no commit"))?;
    let head = git(workspace, &["rev-parse", "--verify", "-q", "HEAD"])
        .map_err(|e| format!("impact: the workspace has no HEAD commit: {e}"))?;
    let base = git(workspace, &["merge-base", &resolved, &head])
        .map_err(|e| format!("impact: no merge base of '{rev}' and HEAD: {e}"))?;
    Ok(Comparison {
        source: "base+diff",
        requested_ref: Some(rev.to_string()),
        resolved_commit: Some(resolved),
        merge_base: Some(base.clone()),
        comparison_base: Some(base),
        comparison_base_note:
            "the merge base of the requested ref and HEAD; the diff's pre-images are checked against it"
                .into(),
        head_commit: Some(head),
        inputs_stamp: None,
        diff_hash: Some(diff_hash(diff)),
        ..Comparison::default()
    })
}

/// A unified-diff section adding `path` with `body` (an untracked file).
fn new_file_section(path: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut s = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"
    );
    if !lines.is_empty() {
        s.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
        for l in lines {
            s.push('+');
            s.push_str(l.trim_end_matches('\r'));
            s.push('\n');
        }
    }
    s
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
