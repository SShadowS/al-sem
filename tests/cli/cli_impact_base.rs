//! `alsem impact --base` with a before/after delta (agent-tasks design §10,
//! milestone B2), through the SHIPPED binary, one honesty rule per test.
//!
//! Each test makes a real git repository in a temporary folder: the base
//! text committed, the change written to the working tree (not committed),
//! then `alsem impact <ws> --base HEAD --no-tests --format json`. The
//! impact-base cache goes to a folder of the test's own
//! (`AL_SEM_IMPACT_BASE_CACHE`), never to the user's. Expected values are
//! hand-stated from the two texts below each test.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const APP_JSON: &str = r#"{
  "id": "7d3c1a20-5b8e-4f61-9a2d-0c4e6b1f8a01",
  "name": "B2 Fixture",
  "publisher": "al-sem tests",
  "version": "1.0.0.0",
  "runtime": "17.0",
  "idRanges": [{"from": 50800, "to": 50849}]
}
"#;

/// The tables every fixture declares.
const TABLES: &str = r#"table 50800 "FX One"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Qty; Decimal) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

table 50801 "FX Two"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}
"#;

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn ws(&self) -> PathBuf {
        self.dir.path().join("ws")
    }
    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }
    fn write(&self, rel: &str, text: &str) {
        let p = self.ws().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

/// `git <args>` in the test's own repository (`tests/common/git.rs`: never a
/// hook's `GIT_DIR`, never the machine's config).
fn git(dir: &Path, args: &[&str]) {
    crate::git::run(dir, args);
}

/// A repository whose top is `ws/`, holding `app.json`, the tables and
/// `src/Core.al` = `base`, committed.
fn repo(base: &str) -> Repo {
    let r = Repo {
        dir: tempfile::tempdir().unwrap(),
    };
    r.write("app.json", APP_JSON);
    r.write("src/Tables.al", TABLES);
    r.write("src/Core.al", base);
    git(&r.ws(), &["init", "-q"]);
    git(&r.ws(), &["add", "."]);
    git(&r.ws(), &["commit", "-qm", "base"]);
    r
}

/// `alsem impact <ws> --base HEAD --no-tests --format json <extra>`.
fn impact(r: &Repo, extra: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg(r.ws())
        .args(["--base", "HEAD", "--no-tests", "--format", "json"])
        .args(extra)
        .env("AL_SEM_IMPACT_BASE_CACHE", r.cache())
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(2),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
}

/// The root of `name` (`…::name`) in the answer.
fn root<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["display"]
                .as_str()
                .is_some_and(|d| d.ends_with(&format!("::{name}")))
        })
        .unwrap_or_else(|| panic!("no root {name}: {v:#}"))
}

/// `(change, label)` of a root's own changes.
fn own(r: &Value) -> Vec<(String, String)> {
    r["delta"]["own"]
        .as_array()
        .unwrap_or_else(|| panic!("no delta: {r:#}"))
        .iter()
        .map(|c| {
            (
                c["change"].as_str().unwrap().to_string(),
                c["label"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect()
}

const BASE_POST: &str = r#"codeunit 50810 "FX Core"
{
    procedure Post()
    var
        One: Record "FX One";
    begin
        One.Modify();
        Commit();
    end;

    procedure Calm()
    begin
        Message('unchanged');
    end;
}
"#;

/// B-3: an added `Insert` on a table shows `+ write … Insert`, a removed
/// `Commit` shows `- commit`, and a routine the change does not touch has no
/// delta (it is not a root at all).
///
/// Break: compare `effects` totals instead of keys → `commits: yes → no`
/// is no key change; the own list is empty.
#[test]
fn b3_added_and_removed_keys() {
    let r = repo(BASE_POST);
    r.write(
        "src/Core.al",
        &BASE_POST.replace(
            "        One.Modify();\n        Commit();\n",
            "        One.Modify();\n        One.Insert();\n",
        ),
    );
    let v = impact(&r, &[]);
    assert_eq!(v["roots"].as_array().unwrap().len(), 1, "only Post changed");
    let post = root(&v, "Post");
    assert_eq!(post["delta"]["pairing"], "id");
    assert_eq!(
        own(post),
        pairs(&[
            ("removed", "commit"),
            ("added", "write FX One Insert (physical)"),
        ])
    );
    assert_eq!(v["behavior"]["behaviorChanged"], 1);
}

/// B-3c: a comment inserted above an unchanged `Commit()` moves it a line
/// down and gives no delta (the routine is a root: its body lines changed).
///
/// Break: key own sites by `file:line` (`Site::ident` = the line) → a
/// removed and an added commit.
#[test]
fn b3c_comment_above_an_unchanged_commit_is_no_delta() {
    let r = repo(BASE_POST);
    r.write(
        "src/Core.al",
        &BASE_POST.replace(
            "        Commit();\n",
            "        // the commit closes the posting\n        Commit();\n",
        ),
    );
    let v = impact(&r, &[]);
    let post = root(&v, "Post");
    assert_eq!(own(post), vec![]);
    assert_eq!(post["delta"]["cone"], serde_json::json!([]));
    assert_eq!(v["behavior"]["behaviorChanged"], 0);
}

/// B-3d: `Modify` on FX One replaced by `Modify` on FX Two (one write
/// before, one after) shows both tables.
///
/// Break: compare write counts per routine → no change.
#[test]
fn b3d_a_count_preserving_table_swap_shows_both() {
    let base = r#"codeunit 50810 "FX Core"
{
    procedure Swap()
    var
        One: Record "FX One";
        Two: Record "FX Two";
    begin
        One.Modify();
    end;
}
"#;
    let r = repo(base);
    r.write(
        "src/Core.al",
        &base.replace("One.Modify();", "Two.Modify();"),
    );
    let v = impact(&r, &[]);
    assert_eq!(
        own(root(&v, "Swap")),
        pairs(&[
            ("removed", "write FX One Modify (physical)"),
            ("added", "write FX Two Modify (physical)"),
        ])
    );
}

/// B-3e (and the incomplete new side): the working tree drops a `Modify`
/// and adds a call the engine cannot resolve: the lost write is "no longer
/// observed", with the reason, not "removed".
///
/// Break: word every loss "removed" (`policy::loss_kind(false)`).
#[test]
fn b3e_a_loss_under_an_incomplete_new_side_is_no_longer_observed() {
    let base = r#"codeunit 50810 "FX Core"
{
    procedure Lossy()
    var
        One: Record "FX One";
    begin
        One.Modify();
    end;
}
"#;
    let r = repo(base);
    r.write(
        "src/Core.al",
        &base.replace(
            "    var\n        One: Record \"FX One\";\n    begin\n        One.Modify();\n",
            "    var\n        One: Record \"FX One\";\n        Ext: Codeunit \"FX Not Loaded\";\n    begin\n        Ext.Handle(One);\n",
        ),
    );
    let v = impact(&r, &[]);
    let lossy = root(&v, "Lossy");
    let changes = lossy["delta"]["own"].as_array().unwrap();
    let lost = changes
        .iter()
        .find(|c| c["label"] == "write FX One Modify (physical)")
        .unwrap_or_else(|| panic!("{lossy:#}"));
    assert_eq!(lost["change"], "no-longer-observed", "{lost:#}");
    assert!(
        lost["reason"].as_str().unwrap().contains("unresolved"),
        "{lost:#}"
    );
    assert!(
        lossy["delta"]["coverage"]["new"]["unresolved"]
            .as_u64()
            .unwrap()
            > 0
    );
}

/// B-4: a routine deleted together with its call is listed as deleted
/// (complete inventories), resolved in the base, with its base caller.
///
/// Break: classify deleted from the new model (no base inventory) → no
/// `deleted` row.
#[test]
fn b4_a_deleted_routine_is_listed_with_its_base_callers() {
    let base = r#"codeunit 50810 "FX Core"
{
    procedure Caller()
    begin
        Gone();
    end;

    procedure Gone()
    var
        One: Record "FX One";
    begin
        One.Delete();
    end;
}
"#;
    let r = repo(base);
    r.write(
        "src/Core.al",
        r#"codeunit 50810 "FX Core"
{
    procedure Caller()
    begin
    end;
}
"#,
    );
    let v = impact(&r, &[]);
    let deleted = v["deleted"].as_array().unwrap();
    assert_eq!(deleted.len(), 1, "{v:#}");
    assert_eq!(deleted[0]["display"], "Codeunit FX Core::Gone");
    assert_eq!(deleted[0]["baseCallers"]["count"], 1);
    assert_eq!(
        deleted[0]["baseCallers"]["first"][0]["display"],
        "Codeunit FX Core::Caller"
    );
    // The caller lost its callee.
    let caller = root(&v, "Caller");
    assert_eq!(
        own(caller),
        pairs(&[("removed", "calls Codeunit FX Core::Gone")])
    );
}

/// B-5: a parameter added to a routine pairs it with its base self by
/// `renames.rs` (`signatureChanged`, pairing `signature`); two overloads
/// changed at once stay unpaired: one deleted, one added, with the reason.
///
/// A caller of the paired routine keeps the same call (its callee key read
/// through the pairing).
///
/// Breaks: pair by name alone → the overloads pair too; compare callee keys
/// without the pairing (`root_delta` for `root_delta_with`) → UsesSig shows
/// a removed and an added call.
#[test]
fn b5_signature_change_pairs_and_changed_overloads_do_not() {
    let base = r#"codeunit 50810 "FX Core"
{
    procedure Sig(A: Integer)
    begin
        Commit();
    end;

    procedure UsesSig()
    begin
        Sig(1);
    end;

    procedure Over(A: Integer)
    begin
    end;

    procedure Over(A: Text)
    begin
    end;
}
"#;
    let r = repo(base);
    r.write(
        "src/Core.al",
        r#"codeunit 50810 "FX Core"
{
    procedure Sig(A: Integer; B: Integer)
    begin
        Commit();
    end;

    procedure UsesSig()
    begin
        Sig(1, 2);
    end;

    procedure Over(A: Decimal)
    begin
    end;

    procedure Over(A: Code[20])
    begin
    end;
}
"#,
    );
    let v = impact(&r, &[]);
    let sig = root(&v, "Sig");
    assert_eq!(sig["status"], "signatureChanged");
    assert_eq!(sig["delta"]["pairing"], "signature");
    assert_eq!(own(sig), vec![], "Sig's behavior is unchanged");
    // The call to Sig is the same call (its callee paired by signature).
    assert_eq!(own(root(&v, "UsesSig")), vec![], "UsesSig still calls Sig");
    let overs: Vec<&Value> = v["roots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["display"] == "Codeunit FX Core::Over")
        .collect();
    assert_eq!(overs.len(), 2);
    for o in overs {
        assert_eq!(o["status"], "added", "{o:#}");
        assert!(
            o["delta"]["reason"]
                .as_str()
                .unwrap()
                .contains("2 routines of the same object and name only on the other side")
        );
    }
    let deleted = v["deleted"].as_array().unwrap();
    assert_eq!(deleted.len(), 2, "{v:#}");
    assert!(
        deleted
            .iter()
            .all(|d| d["display"] == "Codeunit FX Core::Over")
    );
}

/// A workspace in a subfolder of its repository (as CDO's `Cloud`): the
/// base copy holds that folder's files at the commit (`git archive` run at
/// the top; run in the subfolder it keeps nothing of a subfolder tree).
///
/// Break: run `git archive` in the workspace folder → the copy is empty and
/// the base is refused ("holds no app.json").
#[test]
fn a_workspace_in_a_subfolder_of_its_repository() {
    let r = Repo {
        dir: tempfile::tempdir().unwrap(),
    };
    let top = r.dir.path().join("ws");
    let sub = top.join("Cloud");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    std::fs::write(sub.join("app.json"), APP_JSON).unwrap();
    std::fs::write(sub.join("src/Tables.al"), TABLES).unwrap();
    std::fs::write(sub.join("src/Core.al"), BASE_POST).unwrap();
    git(&top, &["init", "-q"]);
    git(&top, &["add", "."]);
    git(&top, &["commit", "-qm", "base"]);
    std::fs::write(
        sub.join("src/Core.al"),
        BASE_POST.replace("Commit();", "Commit();\n        Commit();"),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg(&sub)
        .args(["--base", "HEAD", "--no-tests", "--format", "json"])
        .env("AL_SEM_IMPACT_BASE_CACHE", r.cache())
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    assert!(v["base"]["notComputed"].is_null(), "{:#}", v["base"]);
    let post = root(&v, "Post");
    let changes = post["delta"]["own"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "{post:#}");
    assert_eq!(changes[0]["change"], "count");
    assert_eq!(changes[0]["label"], "commit");
}

/// A routine renamed (its name changed) is not a signature change:
/// `renames.rs` pairs one name only, so it is deleted plus added.
#[test]
fn a_renamed_routine_is_deleted_and_added() {
    let base = r#"codeunit 50810 "FX Core"
{
    procedure OldName()
    begin
        Commit();
    end;
}
"#;
    let r = repo(base);
    r.write("src/Core.al", &base.replace("OldName", "NewName"));
    let v = impact(&r, &[]);
    assert_eq!(root(&v, "NewName")["status"], "added");
    let deleted = v["deleted"].as_array().unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0]["display"], "Codeunit FX Core::OldName");
}

/// B-6: a second run with the same key reads the cache (no base build);
/// another option builds again; a corrupted entry is rebuilt.
///
/// Break: key by commit id only → the `--single-app` run is a hit.
#[test]
fn b6_the_cache_hits_misses_and_rebuilds() {
    let r = repo(BASE_POST);
    r.write(
        "src/Core.al",
        &BASE_POST.replace("Commit();", "Commit();\n        Commit();"),
    );
    let first = impact(&r, &[]);
    assert_eq!(first["base"]["cache"]["status"], "miss");
    assert!(first["base"]["ms"]["build"].is_u64());
    let second = impact(&r, &[]);
    assert_eq!(second["base"]["cache"]["status"], "hit");
    assert!(second["base"]["ms"]["build"].is_null(), "no build on a hit");
    assert_eq!(second["roots"][0]["delta"], first["roots"][0]["delta"]);
    let other = impact(&r, &["--single-app"]);
    assert_eq!(other["base"]["cache"]["status"], "miss");
    // Corrupt every entry: the next run rebuilds.
    for e in std::fs::read_dir(r.cache()).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "ib") {
            let mut b = std::fs::read(&p).unwrap();
            let n = b.len();
            b[n - 5] ^= 0xff;
            std::fs::write(&p, b).unwrap();
        }
    }
    let rebuilt = impact(&r, &[]);
    assert_eq!(rebuilt["base"]["cache"]["status"], "rebuilt");
    assert_eq!(rebuilt["roots"][0]["delta"], first["roots"][0]["delta"]);
}

/// Every file under `dir` with its bytes (`.git` left out).
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.clone(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// B-7: the base never writes into the workspace (every file and byte the
/// same after), reads the CURRENT workspace's packages (the base side read
/// the same package files as the working tree, from the workspace's
/// `.alpackages`), and a changed package is a cache miss.
///
/// Break: read the base's own (empty) package discovery (drop
/// `--package-root`) → the base side reads no package.
#[test]
fn b7_the_base_reads_todays_packages_and_writes_nothing() {
    let r = repo(BASE_POST);
    let pkgs = r.ws().join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r3a5-fixtures/cccccccc-0001-0000-0000-000000000001.app");
    std::fs::copy(&fixture, pkgs.join("dep.app")).unwrap();
    std::fs::write(r.ws().join(".gitignore"), ".alpackages/\n").unwrap();
    r.write(
        "src/Core.al",
        &BASE_POST.replace("Commit();", "Commit();\n        Commit();"),
    );
    let before = tree(&r.ws());
    let v = impact(&r, &[]);
    assert_eq!(tree(&r.ws()), before, "the workspace is unchanged");
    let read = &v["base"]["packagesRead"];
    assert_eq!(read["base"].as_array().unwrap().len(), 1, "{read:#}");
    assert_eq!(read["same"], true);
    assert!(read["base"][0].as_str().unwrap().ends_with("dep.app"));
    // Another package: another key.
    let other = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r3a5-fixtures/55555555-0005-0000-0000-000000000001.app");
    std::fs::copy(&other, pkgs.join("dep.app")).unwrap();
    let again = impact(&r, &[]);
    assert_eq!(again["base"]["cache"]["status"], "miss");
}

/// A diff whose pre-images are not the base's text is refused with the
/// files named; the current side still answers.
#[test]
fn a_diff_not_matching_the_base_is_refused() {
    let r = repo(BASE_POST);
    let diff = "diff --git a/src/Core.al b/src/Core.al\n--- a/src/Core.al\n+++ b/src/Core.al\n@@ -7,2 +7,2 @@\n-        One.Insert();\n+        One.Modify();\n         Commit();\n";
    let p = r.dir.path().join("change.diff");
    std::fs::write(&p, diff).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg(r.ws())
        .args(["--base", "HEAD", "--diff"])
        .arg(&p)
        .args(["--no-tests", "--format", "json"])
        .env("AL_SEM_IMPACT_BASE_CACHE", r.cache())
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let why = v["base"]["notComputed"]
        .as_str()
        .unwrap_or_else(|| panic!("{v:#}"));
    assert!(why.contains("pre-images do not match"), "{why}");
    assert!(why.contains("src/Core.al"), "{why}");
    assert!(
        !v["roots"].as_array().unwrap().is_empty(),
        "the current side stands"
    );
}

/// `(kind, normalizedStableId)` of the schema findings of a findings list.
fn schema_findings(findings: &Value) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = findings
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["category"] == "schema")
        .map(|f| {
            (
                f["kind"].as_str().unwrap().to_string(),
                f["subject"]["normalizedStableId"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

/// B-9: impact's schema findings are `alsem diff <base> <new> --format
/// json`'s (one shared engine), the table key passes included: a key's
/// fields changed, a key added, a field added and a field's type changed.
///
/// Break: drop the key passes from `diff_schema` → the key findings vanish
/// from both, and the hand-stated list fails.
#[test]
fn b9_schema_findings_are_alsem_diffs_keys_included() {
    let r = repo(BASE_POST);
    // The base, as a plain folder for `alsem diff`.
    let old = r.dir.path().join("old");
    crate::copy_dir(&r.ws(), &old);
    let _ = std::fs::remove_dir_all(old.join(".git"));
    r.write(
        "src/Tables.al",
        &TABLES
            .replace(
                "        field(2; Qty; Decimal) { }\n",
                "        field(2; Qty; Integer) { }\n        field(3; Note; Text[50]) { }\n",
            )
            .replace(
                "        key(PK; \"No.\") { Clustered = true; }\n",
                "        key(PK; \"No.\", Qty) { Clustered = true; }\n        key(ByNote; Note) { }\n",
            ),
    );
    r.write(
        "src/Core.al",
        &BASE_POST.replace("Commit();", "Commit();\n        Commit();"),
    );
    let v = impact(&r, &[]);
    let mine = schema_findings(&v["schemaChanges"]["findings"]);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["diff"])
        .arg(&old)
        .arg(r.ws())
        .args(["--format", "json"])
        .output()
        .unwrap();
    let d: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let theirs = schema_findings(&d["payload"]["findings"]);
    assert_eq!(mine, theirs);
    let t = "7d3c1a20-5b8e-4f61-9a2d-0c4e6b1f8a01:Table:50800";
    assert_eq!(
        mine,
        vec![
            ("table-field-added".to_string(), format!("{t}#3")),
            ("table-field-type-narrowed".to_string(), format!("{t}#2")),
            ("table-key-added".to_string(), format!("{t}#K1")),
            ("table-key-fields-changed".to_string(), format!("{t}#K0")),
        ]
    );
}

/// The dependency declaration changed: the label says so and the base
/// block names the change.
#[test]
fn a_changed_dependency_declaration_is_named() {
    let r = repo(BASE_POST);
    r.write(
        "app.json",
        &APP_JSON.replace(
            "\"idRanges\"",
            "\"dependencies\": [{\"id\": \"cccccccc-0001-0000-0000-000000000001\", \"name\": \"Dep\", \"publisher\": \"x\", \"version\": \"1.0.0.0\"}],\n  \"idRanges\"",
        ),
    );
    r.write(
        "src/Core.al",
        &BASE_POST.replace("Commit();", "Commit();\n        Commit();"),
    );
    let v = impact(&r, &[]);
    let changed = v["base"]["dependencyDeclarationChanged"]
        .as_array()
        .unwrap();
    assert_eq!(changed.len(), 1, "{:#}", v["base"]);
    assert!(changed[0].as_str().unwrap().starts_with("added Dep"));
}

/// B2's CDO acceptance (`scripts/cdo-gate`, via `output_cdo`): `--base
/// bc3ccb18~5` on the pinned baseline (git is only read; the base copy and
/// the cache go to a temporary folder). Hand-checked against both commits'
/// source (2026-11-05):
/// - `CDO Ver. 2.0 Convert::ConvertTemplate`: base line 158
///   `EMailTemplateLine.Insert()` became line 160
///   `DocumentTemplateLine.Insert(true)`: a count-preserving table swap,
///   both tables shown;
/// - `CDO Azure Blob Storage::FileExists`: line 30 now calls `ResolvePath`;
/// - `Table CDO E-Mail Log::InitNewEntry`: its body moved into
///   `InitNewEntryCore` (added call) and no longer calls `SetTo`/`SetCc`/
///   `SetBcc` itself (removed calls);
/// - the base: 11 package files, the same on both sides, and app.json's
///   dependency versions 29 → 30 named.
pub(crate) fn cdo_impact_base_ratchets() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg(&ws)
        .args(["--base", "bc3ccb18~5", "--no-tests", "--format", "json"])
        .env("AL_SEM_IMPACT_BASE_CACHE", cache.path())
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    assert!(v["base"]["notComputed"].is_null(), "{:#}", v["base"]);
    assert_eq!(
        v["base"]["environment"]["packages"]
            .as_array()
            .unwrap()
            .len(),
        11
    );
    assert_eq!(v["base"]["packagesRead"]["same"], true);
    assert_eq!(
        v["base"]["dependencyDeclarationChanged"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    // Every root of that name (an overloaded name has one per overload).
    let changes = |name: &str| -> Vec<(String, String)> {
        v["roots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                r["display"]
                    .as_str()
                    .is_some_and(|d| d.ends_with(&format!("::{name}")))
                    && r["delta"]["own"].is_array()
            })
            .flat_map(own)
            .collect()
    };
    let convert = changes("ConvertTemplate");
    for want in [
        (
            "removed",
            "write CDO E-Mail Template Line Insert (physical)",
        ),
        (
            "added",
            "write CDO Document Template Line Insert (physical)",
        ),
    ] {
        assert!(
            convert.contains(&(want.0.to_string(), want.1.to_string())),
            "{convert:?}"
        );
    }
    assert_eq!(
        changes("FileExists"),
        pairs(&[(
            "added",
            "calls Codeunit CDO Azure Blob Storage::ResolvePath"
        )])
    );
    let init = changes("InitNewEntry");
    for want in [
        ("added", "calls Table CDO E-Mail Log::InitNewEntryCore"),
        ("removed", "calls Table CDO E-Mail Log::SetTo"),
        ("removed", "calls Table CDO E-Mail Log::SetCc"),
        ("removed", "calls Table CDO E-Mail Log::SetBcc"),
    ] {
        assert!(
            init.contains(&(want.0.to_string(), want.1.to_string())),
            "{init:?}"
        );
    }
}
