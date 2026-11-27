//! ⟨TLS-61⟩ `scripts/cdo-before-after`: what a change moves on CDO, base against
//! head. Every test runs in a throwaway checkout (the three scripts copied in, two
//! commits), with a stub `cargo` that "builds" stub `alsem` and `aldump` binaries
//! answering from per-side fixture files, its own speed cache
//! (`ALSEM_SPEED_CACHE`), slot folder (`CDO_SLOT_DIR`) and workspace. Nothing
//! touches the real checkout, cache, slot or CDO.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

// cdo-slot sources awake-clock (TLS-105).
const SCRIPTS: [&str; 4] = ["cdo-before-after", "cdo-speed", "cdo-slot", "awake-clock"];

/// A stub cargo. `build`: one stub per `--bin`, with the source's `side.txt`
/// baked in (so two sides' binaries differ exactly when their `side.txt` does).
/// `test --no-run` succeeds; `test` prints `census.txt` as ENG14 census lines and
/// fails like a moved ratchet would. Every call is logged.
const CARGO: &str = r#"#!/bin/sh
echo "cargo $* | $CARGO_TARGET_DIR | $PWD" >> "$BA_LOG"
for f in "$CARGO_TARGET_DIR"/.before-after*.lock; do
  [ -e "$f" ] || continue
  if flock -n "$f" true; then echo "lock $(basename "$f") free"; else echo "lock $(basename "$f") held"; fi >> "$BA_LOG"
done
[ -n "$BA_CARGO_FAIL" ] && exit 1
side=$(cat side.txt)
case "$1" in
build)
  mkdir -p "$CARGO_TARGET_DIR/release-fast"
  prev=""
  for a in "$@"; do
    if [ "$prev" = --bin ]; then
      printf '#!/bin/sh\nexec "%s" %s %s "$@"\n' "$BA_STUB" "$a" "$side" > "$CARGO_TARGET_DIR/release-fast/$a"
      chmod +x "$CARGO_TARGET_DIR/release-fast/$a"
    fi
    prev=$a
  done ;;
test)
  case " $* " in *" --no-run "*) ;; *) echo "cargo test at measure time" >&2; exit 1 ;; esac
  exe="${CARGO_TARGET_DIR:-$PWD/target}/cdo/deps/al_sem-test"
  mkdir -p "$(dirname "$exe")"
  {
    printf '#!/bin/sh\necho "census-bin $0 $*" >> "$BA_LOG"\nF="$BA_FIX/%s"\n' "$side"
    printf '[ -f "$F/no-census" ] && exit 101\n'
    sed "s/^/echo 'ENG14-CENSUS /; s/\$/'/" census.txt
    printf '[ -f "$F/census-panic" ] && { echo "panicked: something else" >&2; exit 101; }\n'
    printf 'echo "untrusted routines per app moved: explain the change, then re-pin" >&2\nexit 101\n'
  } > "$exe"
  chmod +x "$exe"
  echo '{"reason":"compiler-artifact","target":{"kind":["bin"]},"profile":{"test":true},"executable":"/nonexistent"}'
  printf '{"reason":"compiler-artifact","target":{"kind":["lib"]},"profile":{"test":true},"executable":"%s"}\n' "$exe"
  echo '{"reason":"build-finished","success":true}' ;;
esac
"#;

/// The stub behind every built binary: `<tool> <side> <args>`, answering from
/// `$BA_FIX/<side>/`. `fail-<tool>-<first arg>` there makes that command exit 3;
/// `dirty-ws` makes `aldump` leave a file in the workspace (the input moving).
const STUB: &str = r#"#!/bin/sh
tool=$1 side=$2
shift 2
echo "$tool $side $*" >> "$BA_LOG"
F="$BA_FIX/$side"
[ -f "$F/fail-$tool-$1" ] && { echo "stub failure" >&2; exit 3; }
case "$tool $1" in
"alsem --version") echo "alsem stub $side" ;;
"alsem fingerprint") echo "slot: $(bash "$BA_SLOT" status 2>&1)" >> "$BA_LOG"; while [ $# -gt 0 ]; do [ "$1" = --out ] && cp "$F/fingerprint.json" "$2"; shift; done ;;
"alsem analyze") case " $* " in *" --scope all "*) cat "$F/analyze-all.json" ;; *) cat "$F/analyze-primary.json" ;; esac ;;
"aldump --program-call-graph-stats") cat "$F/stats.json"; [ -f "$F/dirty-ws" ] && touch "$2/moved.txt"; [ -f "$F/edit-ws" ] && echo again >> "$2/a.txt" ;;
esac
exit 0
"#;

fn findings(rows: &[(&str, &str)]) -> Value {
    let rows: Vec<Value> = rows
        .iter()
        .map(|(d, f)| json!({"detector": d, "fingerprint": f}))
        .collect();
    json!({"payload": {"findings": rows}})
}

struct Ba {
    dir: tempfile::TempDir,
    c1: String,
    c2: String,
}

impl Ba {
    /// Commit c1 has side A (census "app X: 9"), c2 side B ("app X: 8").
    fn new() -> Ba {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = |r: &str| dir.path().join(r);
        for r in ["repo/scripts", "grammar", "ws", "bin", "fix"] {
            std::fs::create_dir_all(p(r)).unwrap();
        }
        for s in SCRIPTS {
            let to = p(&format!("repo/scripts/{s}"));
            std::fs::copy(format!("{}/scripts/{s}", env!("CARGO_MANIFEST_DIR")), &to).unwrap();
        }
        std::fs::write(p("bin/cargo"), CARGO).unwrap();
        std::fs::write(p("stub"), STUB).unwrap();
        for f in ["bin/cargo", "stub"] {
            std::fs::set_permissions(p(f), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let side = |s: &str, primary: &[(&str, &str)], fp: &[&str], unknown: u32| {
            let d = p(&format!("fix/{s}"));
            std::fs::create_dir_all(&d).unwrap();
            let mut all = primary.to_vec();
            all.push(("d4", "f9"));
            let write = |n: &str, v: Value| std::fs::write(d.join(n), v.to_string()).unwrap();
            write("analyze-primary.json", findings(primary));
            write("analyze-all.json", findings(&all));
            write("fingerprint.json", json!({ "ids": fp }));
            write(
                "stats.json",
                json!({
                    "wholeProgram": {"total": 10, "unknown": unknown,
                        "realUnknownRate": f64::from(unknown) / 10.0,
                        "unknownByReason": {"r1": unknown}},
                    "primaryScoped": {"total": 4, "unknown": 1, "realUnknownRate": 0.25},
                }),
            );
        };
        side(
            "A",
            &[("d1", "f1"), ("d1", "f2"), ("d2", "f3")],
            &["X:Interface:Foo", "X:Interface:Gone"],
            5,
        );
        for s in ["B", "C"] {
            side(
                s,
                &[("d1", "f1"), ("d3", "f4")],
                &["X:Interface:Foo", "X:Interface:New"],
                3,
            );
        }
        let ba = Ba {
            dir,
            c1: String::new(),
            c2: String::new(),
        };
        for r in ["grammar", "ws"] {
            std::fs::write(ba.path(r).join("a.txt"), "one\n").unwrap();
            ba.commit(r);
        }
        let c1 = ba.commit_side("A", "untrusted in app X: 9");
        let c2 = ba.commit_side("B", "untrusted in app X: 8");
        Ba { c1, c2, ..ba }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn git(&self, repo: &str, args: &[&str]) -> String {
        let out = crate::git::run(&self.path(repo), args);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn commit(&self, repo: &str) -> String {
        if !self.path(repo).join(".git").exists() {
            self.git(repo, &["init", "-q"]);
        }
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "-q", "-m", "c"]);
        self.git(repo, &["rev-parse", "HEAD"])
    }

    /// A repo commit whose sources build side `side` with census line `census`.
    fn commit_side(&self, side: &str, census: &str) -> String {
        std::fs::write(self.path("repo/side.txt"), format!("{side}\n")).unwrap();
        std::fs::write(self.path("repo/census.txt"), format!("{census}\n")).unwrap();
        self.commit("repo")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.path("calls.log")).unwrap_or_default()
    }

    fn clear_log(&self) {
        let _ = std::fs::remove_file(self.path("calls.log"));
    }

    /// `cdo-before-after --ws <ws> <args>` in the throwaway: (exit, output).
    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> (Option<i32>, String) {
        let mut cmd = std::process::Command::new("python3");
        cmd.arg(self.path("repo/scripts/cdo-before-after"))
            .arg("--ws")
            .arg(self.path("ws"))
            .args(args)
            .current_dir(self.path("repo"))
            .env_clear()
            .env("HOME", self.path("home"))
            .env("ALSEM_SPEED_CACHE", self.path("cache"))
            .env("CARGO_TARGET_DIR", self.path("target"))
            .env("TREE_SITTER_AL_PATH", self.path("grammar"))
            .env("CDO_SLOT_DIR", self.path("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            // TLS-89: its hold is timed; no real build of this machine may hold it up.
            .env("CDO_SLOT_BUILD_NAMES", "tls89-none")
            .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
            .env("BA_LOG", self.path("calls.log"))
            .env("BA_FIX", self.path("fix"))
            .env("BA_STUB", self.path("stub"))
            .env("BA_SLOT", self.path("repo/scripts/cdo-slot"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.path("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            );
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn python3 cdo-before-after");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }

    fn report(&self, out: &str) -> Value {
        let text = std::fs::read_to_string(self.path(out).join("report.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn out(&self, name: &str) -> String {
        self.path(name).to_str().unwrap().to_string()
    }
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// Two commits, every measurement, `--census` and a `--pattern`: the report
/// names exactly what moved, the cache keeps both binaries per commit, a second
/// run builds nothing, and `--dry-run` runs nothing and says what is cached.
///
/// Mutations (2026-11-15), each red here, green reverted: `cached_binary`
/// copying only its first missing binary (`missing[:1]`) -> the first run exits
/// 1 (no aldump); its `missing` list ignoring `extra` (`("alsem",)`) -> the
/// first run exits 1 (aldump never built); it building every binary, not only
/// the missing ones -> the aldump-only build check fails; every detector taken
/// as moved (`if True`) -> `moved` holds d4; `flatten` skipping nested numbers
/// -> `unknownByReason.r1` missing; the census sides swapped -> added and
/// removed swapped; r4 (review r2, sol): the census run with `cargo test` at
/// measure time again -> the stub refuses it, no census line, exit 1 (here, in
/// the working-tree test and in the census refusals).
#[test]
fn before_after_reports_what_moved_and_caches_both_binaries() {
    let t = Ba::new();
    let out = t.out("out1");
    let (code, text) = t.run(
        &[
            "--census",
            "--pattern",
            ":Interface:[A-Za-z]*",
            "--out",
            &out,
            &t.c1,
            &t.c2,
        ],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    let rep = t.report("out1");
    assert_eq!(rep["void"], Value::Null);
    let primary = &rep["findings"]["analyze-primary"];
    assert_eq!(primary["total"], json!([3, 2]));
    assert_eq!(
        primary["moved"],
        json!({"d1": [2, 1], "d2": [1, 0], "d3": [0, 1]})
    );
    assert_eq!(
        primary["fingerprints"],
        json!({"added": ["f4"], "removed": ["f2", "f3"], "kept": 1})
    );
    assert_eq!(
        rep["findings"]["analyze-all"]["moved"],
        json!({"d1": [2, 1], "d2": [1, 0], "d3": [0, 1]})
    );
    assert_eq!(rep["findings"]["analyze-all"]["fingerprints"]["kept"], 2);
    let whole = &rep["histogram"]["wholeProgram"];
    assert_eq!(whole["unknown"], json!([5, 3]));
    assert_eq!(whole["realUnknownRate"], json!([0.5, 0.3]));
    assert_eq!(
        whole["moved"],
        json!({"realUnknownRate": [0.5, 0.3], "unknown": [5, 3], "unknownByReason.r1": [5, 3]})
    );
    assert_eq!(rep["histogram"]["primaryScoped"]["moved"], json!({}));
    assert_eq!(
        rep["patterns"][":Interface:[A-Za-z]*"],
        json!({"occurrences": [2, 2], "distinct": [2, 2],
               "added": [":Interface:New"], "removed": [":Interface:Gone"]})
    );
    assert_eq!(
        rep["census"],
        json!({"added": ["ENG14-CENSUS untrusted in app X: 8"],
               "removed": ["ENG14-CENSUS untrusted in app X: 9"]})
    );
    let md = std::fs::read_to_string(t.path("out1/report.md")).unwrap();
    for line in [
        "- d2: 1 -> 0",
        "unknown 5 -> 3, realUnknownRate 0.5 -> 0.3",
        "- + ENG14-CENSUS untrusted in app X: 8",
    ] {
        assert!(md.contains(line), "{line} not in:\n{md}");
    }
    // Base measured before head, each command once per side.
    let log = t.log();
    let calls: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("alsem ") || l.starts_with("aldump "))
        .filter(|l| !l.contains("--version"))
        .map(|l| &l[..l.find(' ').unwrap() + 2])
        .collect();
    assert_eq!(
        calls,
        [
            "alsem A", "alsem A", "alsem A", "aldump A", "alsem B", "alsem B", "alsem B",
            "aldump B"
        ],
        "{log}"
    );
    // The census ran from each side's copied test executable (review r2, sol).
    for side in ["base", "head"] {
        let bin = format!(
            "census-bin {}",
            t.path(&format!("out1/census-bin/{side}")).display()
        );
        assert!(log.contains(&bin), "{bin} not in {log}");
    }
    // Each side measured while this run held the slot.
    let slot: Vec<&str> = log.lines().filter(|l| l.starts_with("slot: ")).collect();
    assert_eq!(slot.len(), 2, "{log}");
    assert!(
        slot.iter()
            .all(|l| l.starts_with("slot: held by before-after")),
        "{log}"
    );
    // TLS-89: and the hold was timed (no build may run beside a measurement).
    // Mutation (2026-11-18): `--timed` dropped from `take_slot` -> no `timed by` line (red).
    assert_eq!(
        log.lines()
            .filter(|l| l.starts_with("timed by before-after since"))
            .count(),
        2,
        "{log}"
    );
    // One cache: both binaries kept per commit, beside cdo-speed's alsem.
    for c in [&t.c1, &t.c2] {
        for b in ["alsem", "aldump"] {
            assert!(exists(&t.path(&format!("cache/bin/{c}/{b}"))), "{b} of {c}");
        }
    }

    // A second run builds no binary; with only cdo-speed's alsem cached for a
    // commit, only its aldump is built.
    std::fs::remove_file(t.path(&format!("cache/bin/{}/aldump", t.c1))).unwrap();
    t.clear_log();
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    let builds: Vec<String> = t
        .log()
        .lines()
        .filter(|l| l.starts_with("cargo build"))
        .map(|l| l.split(" | ").next().unwrap().to_string())
        .collect();
    assert_eq!(
        builds,
        ["cargo build --profile release-fast --bin aldump"],
        "{}",
        t.log()
    );
    t.clear_log();
    let out2 = t.out("out2");
    let (code, text) = t.run(&["--out", &out2, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(!t.log().contains("cargo build"), "{}", t.log());
    assert!(
        !t.log().contains("cargo test"),
        "census is off by default: {}",
        t.log()
    );

    // --dry-run: no build, no measurement, the cache named.
    t.clear_log();
    let (code, text) = t.run(&["--dry-run", &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(t.log(), "", "dry run ran something");
    assert!(text.contains("cached in"), "{text}");
}

/// The head side defaults to `.`, the working tree with its edits, built in
/// `<target>/before-after-head` (never the cache, never the main target).
///
/// Mutations (2026-11-15): `build_side`'s `.` branch with no `CARGO_TARGET_DIR`
/// of its own (`env = dict(os.environ)`) -> it builds into the main target and
/// the run exits 1 (no binary in `before-after-head`); r2 (review r1, sol): it
/// returning the shared target instead of its copy (`return built`) -> the
/// head's binary path is not `out/head-bin/alsem`; r4 (review r3, sol): `out`
/// not made absolute -> the relative `--out`'s census executable is not found
/// (exit 1); the census build's lock not taken -> the stub reports
/// `.before-after-census.lock` free; the head build's lock not taken -> it
/// reports `.before-after.lock` free.
#[test]
fn before_after_head_defaults_to_the_working_tree() {
    let t = Ba::new();
    std::fs::write(t.path("repo/side.txt"), "C\n").unwrap();
    std::fs::write(t.path("repo/census.txt"), "untrusted in app X: 7\n").unwrap();
    // A relative --out (the run's folder is the repo).
    let (code, text) = t.run(&["--census", "--out", "out", &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    std::fs::rename(t.path("repo/out"), t.path("out")).unwrap();
    assert_eq!(
        t.report("out")["census"]["added"],
        json!(["ENG14-CENSUS untrusted in app X: 7"])
    );
    // Both working-tree builds ran holding their target's lock (review r3, sol).
    let log = t.log();
    for lock in [".before-after.lock", ".before-after-census.lock"] {
        assert!(log.contains(&format!("lock {lock} held")), "{lock}: {log}");
    }
    assert!(!log.contains(" free"), "{log}");
    let head_target = format!("| {} |", t.path("target/before-after-head").display());
    assert!(
        t.log()
            .lines()
            .any(|l| l.starts_with("cargo build") && l.contains(&head_target)),
        "{}",
        t.log()
    );
    assert!(t.log().contains("alsem C fingerprint"), "{}", t.log());
    let rep = t.report("out");
    // Measured from this run's own copy, not the shared target (review r1, sol).
    assert_eq!(
        rep["head"]["alsem"],
        json!(t.path("repo/out/head-bin/alsem").to_str().unwrap()),
    );
    assert_eq!(rep["findings"]["analyze-primary"]["moved"], json!({}));
}

/// The default output folder under a relative `CARGO_TARGET_DIR` (the run's
/// folder is the repo) still works with `--census`, whose base side runs from
/// its archived sources.
///
/// Mutation (2026-11-15, review r4, sol): the default `out` not made absolute
/// (`abspath` only around `a.out`) -> the base census executable is not found,
/// exit 1.
#[test]
fn before_after_default_out_under_a_relative_target() {
    let t = Ba::new();
    let (code, text) = t.run(
        &["--census", &t.c1, &t.c2],
        &[("CARGO_TARGET_DIR", "rel-target")],
    );
    assert_eq!(code, Some(0), "{text}");
    let runs: Vec<_> = std::fs::read_dir(t.path("repo/rel-target/before-after"))
        .unwrap()
        .collect();
    assert_eq!(runs.len(), 1);
    let run = runs[0].as_ref().unwrap().path();
    assert!(run.join("report.md").exists(), "{}", run.display());
}

/// Every refusal exits non-zero and says why: the same tree (2); binaries that
/// came out identical (1, nothing measured); a command that fails on base (head
/// never measured); a census with no census line, or one that panicked for
/// another reason than its ratchet; a histogram missing a section; a workspace
/// that moved during the base or the head side (VOID, 1, the report written);
/// the working tree's build failing (and a reused --out's old report removed).
///
/// Mutations (2026-11-15), each failing its own case: the same-tree check off
/// -> that run exits 1 (identical binaries) instead of 2; the binary sha256
/// check off -> the identical-binaries run measures and exits 0; `measure`'s
/// `if rc != 0` off -> the failed command is not named (the run dies later on
/// the empty output); the census `if not lines` off -> exits 0; r2 (review r1,
/// Fable): the census `not whole` off -> the panicked census exits 0; the
/// missing-`unknown` check off -> exits 0; the after-side workspace reads left
/// out of `reads` -> the head-side move exits 0; the working tree's `if rc !=
/// 0` off -> not named (dies on the missing binary); the old-report removal off
/// -> the VOID report.json is still there; r3 (review r1, sol): the edits hash
/// left out of `reads` -> the same-count edit exits 0; the `--version` exit
/// check off -> the failed `--version` is not named; r4 (review r2, sol): the
/// --out lock not taken -> the held --out runs and exits 0.
#[test]
fn before_after_refuses_what_it_cannot_compare() {
    let t = Ba::new();
    let out = t.out("out");

    let (code, text) = t.run(&["--out", &out, &t.c2, &t.c2], &[]);
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("have the same source tree"), "{text}");
    let (code, text) = t.run(&["--dry-run", &t.c2, &t.c2], &[]);
    assert_eq!(code, Some(2), "{text}");

    // c3: other sources, the same binaries (side B again).
    std::fs::write(t.path("repo/other.txt"), "x\n").unwrap();
    let c3 = t.commit("repo");
    t.clear_log();
    let (code, text) = t.run(&["--out", &out, &t.c2, &c3], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("the two alsem binaries are the same"),
        "{text}"
    );
    assert!(!t.log().contains("alsem B fingerprint"), "{}", t.log());

    std::fs::write(t.path("fix/A/fail-alsem-analyze"), "").unwrap();
    t.clear_log();
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("base: analyze-primary exited 3") && text.contains("stub failure"),
        "{text}"
    );
    assert!(!t.log().contains("alsem B"), "head measured: {}", t.log());
    std::fs::remove_file(t.path("fix/A/fail-alsem-analyze")).unwrap();

    std::fs::write(t.path("fix/A/no-census"), "").unwrap();
    let (code, text) = t.run(&["--census", "--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("base: the census printed no ENG14-CENSUS line"),
        "{text}"
    );
    std::fs::remove_file(t.path("fix/A/no-census")).unwrap();
    // A census that panics for another reason than its ratchet is partial.
    std::fs::write(t.path("fix/B/census-panic"), "").unwrap();
    let (code, text) = t.run(&["--census", "--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("head: the census did not finish"), "{text}");
    std::fs::remove_file(t.path("fix/B/census-panic")).unwrap();

    // A histogram with a section missing is no comparison.
    let stats = t.path("fix/B/stats.json");
    let kept = std::fs::read_to_string(&stats).unwrap();
    std::fs::write(&stats, r#"{"wholeProgram": {"unknown": 3}}"#).unwrap();
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("head: the call-graph stats have no primaryScoped.unknown"),
        "{text}"
    );
    std::fs::write(&stats, kept).unwrap();

    // The workspace moves during the base side, then during the head side (its
    // last command): both void the report.
    for side in ["A", "B"] {
        std::fs::write(t.path(&format!("fix/{side}/dirty-ws")), "").unwrap();
        let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
        assert_eq!(code, Some(1), "side {side}: {text}");
        assert!(text.contains("VOID: the workspace moved"), "{text}");
        assert!(t.report("out")["void"].is_string());
        std::fs::remove_file(t.path(&format!("fix/{side}/dirty-ws"))).unwrap();
        std::fs::remove_file(t.path("ws/moved.txt")).unwrap();
    }

    // A file already dirty changes again: the dirty count stays, the edits hash
    // does not (review r1, sol).
    std::fs::write(t.path("ws/a.txt"), "dirty\n").unwrap();
    std::fs::write(t.path("fix/A/edit-ws"), "").unwrap();
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("VOID: the workspace moved"), "{text}");
    std::fs::remove_file(t.path("fix/A/edit-ws")).unwrap();
    t.git("ws", &["checkout", "--", "a.txt"]);

    // Another run holding the same --out (review r2, sol).
    std::fs::create_dir_all(t.path("out")).unwrap();
    // `-o`: only flock holds the lock (not its sleep), so killing it frees the lock.
    let mut holder = std::process::Command::new("flock")
        .arg("-o")
        .arg(t.path("out/.before-after.lock"))
        .args(["sleep", "60"])
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let mut code_text = (None, String::new());
    while start.elapsed() < std::time::Duration::from_secs(10) {
        code_text = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
        if code_text
            .1
            .contains("another cdo-before-after run is using")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = holder.kill();
    let _ = holder.wait();
    let (code, text) = code_text;
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("another cdo-before-after run is using"),
        "{text}"
    );

    // `alsem --version` failing is a failed step too.
    std::fs::write(t.path("fix/A/fail-alsem---version"), "").unwrap();
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("base: alsem --version exited 3"), "{text}");
    std::fs::remove_file(t.path("fix/A/fail-alsem---version")).unwrap();

    // The working tree's build fails (the base is cached by now): named, and the
    // earlier VOID report in the reused --out is gone.
    let (code, text) = t.run(&["--out", &out, &t.c1, "."], &[("BA_CARGO_FAIL", "1")]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("the working tree's build failed"), "{text}");
    assert!(!t.path("out/report.json").exists() && !t.path("out/report.md").exists());
}

/// ⟨TLS-92⟩ Moves of fingerprint MULTIPLICITY are reported. Base and head have
/// the same fingerprints `fx`, `fy` and `fw` and the same detector, so before
/// TLS-92 the report read "nothing moved" for them:
/// - `fx` goes from 2 findings to 1 and `fy` from 1 to 2 (a merge under one
///   fingerprint, a split under another);
/// - `fw` keeps 2 findings but swaps one member (`w2` for `w3`);
/// - `fn` is added on head with 2 findings, which a kept-only check missed
///   (review r1, Fable: ENG-101's fix B splits a group into new fingerprints);
/// - the singleton `fs` is kept and must stay out of both lists.
///
/// Mutations (2026-11-18): `multiplicity_moved` over `pb - ph` instead of
/// `pb | ph` -> empty (red). Review r1: over `pb & ph` (kept only) -> no `fn`
/// (red); the members comparison dropped -> no `fw` (red). Each restored, green.
#[test]
fn before_after_reports_a_multiplicity_change() {
    let t = Ba::new();
    let rows = |r: &[(&str, &str)]| -> String {
        let r: Vec<Value> = r
            .iter()
            .map(|(fp, id)| json!({"detector": "d55", "fingerprint": fp, "id": id}))
            .collect();
        json!({"payload": {"findings": r}}).to_string()
    };
    let base = rows(&[
        ("fx", "x1"),
        ("fx", "x2"),
        ("fy", "y1"),
        ("fw", "w1"),
        ("fw", "w2"),
        ("fs", "s1"),
    ]);
    let head = rows(&[
        ("fx", "x1"),
        ("fy", "y1"),
        ("fy", "y2"),
        ("fw", "w1"),
        ("fw", "w3"),
        ("fs", "s1"),
        ("fn", "n1"),
        ("fn", "n2"),
    ]);
    for (side, v) in [("A", &base), ("B", &head)] {
        std::fs::write(t.path(&format!("fix/{side}/analyze-primary.json")), v).unwrap();
        std::fs::write(t.path(&format!("fix/{side}/analyze-all.json")), v).unwrap();
    }
    let out = t.out("out");
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    let rep = t.report("out");
    for scope in ["analyze-primary", "analyze-all"] {
        let f = &rep["findings"][scope];
        assert_eq!(f["total"], json!([6, 8]), "{scope}: {f}");
        assert_eq!(f["distinct"], json!([4, 5]), "{scope}: {f}");
        assert_eq!(f["fingerprints"]["added"], json!(["fn"]), "{scope}: {f}");
        assert_eq!(
            f["groups"],
            json!({"base": [["d55", "fw", 2], ["d55", "fx", 2]],
                   "head": [["d55", "fn", 2], ["d55", "fw", 2], ["d55", "fy", 2]]}),
            "{scope}: {f}"
        );
        assert_eq!(
            f["multiplicity_moved"],
            json!([
                ["d55", "fn", 0, 2],
                ["d55", "fx", 2, 1],
                ["d55", "fy", 1, 2]
            ]),
            "{scope}: {f}"
        );
        assert_eq!(
            f["members_moved"],
            json!([["d55", "fw", 2]]),
            "{scope}: {f}"
        );
    }
    let md = std::fs::read_to_string(t.path("out/report.md")).unwrap();
    assert!(
        md.contains("**finding count changed** under 3 shared fingerprint(s)")
            && md.contains("d55 fx: 2 -> 1 findings")
            && md.contains("d55 fn: 0 -> 2 findings")
            && md.contains("**finding ids changed** under 1 shared fingerprint(s)")
            && md.contains("d55 fw: 2 findings")
            && md.contains("distinct 4 -> 5; shared by more than one finding: 2 -> 3"),
        "{md}"
    );
}

/// ⟨TLS-92 review r1, sol⟩ The cases a fingerprint-set comparison alone cannot
/// see, each on its own:
/// - same fingerprint set, same total, same detector count: `fa` 2 -> 1 and
///   `fb` 1 -> 2;
/// - a group of 2 split into two NEW singletons (ENG-101's fix B): `fz` 2 -> 0;
/// - a group whose findings have no `id` (`fq`, `A,B -> A,C`): its members are
///   reported unknown, never unchanged;
/// - a fingerprint carrying another detector on head (`fd`: d55 -> d54): flagged.
///
/// Mutations (2026-11-18): the `no_id` exclusion dropped -> `fq` reads as an
/// unchanged group (no `members_unknown`; red); the detector set reduced to
/// one side's detector -> no `detector_mismatch` (red). Each restored, green.
/// Review r2 (2026-11-19): `multiplicity_moved` computed only when the
/// fingerprint set changed (`pb != ph and ...`) -> the same-set run misses
/// `fa` 2 -> 1 and `fb` 1 -> 2 (red); restored, green.
#[test]
fn before_after_reports_what_the_fingerprint_set_hides() {
    let t = Ba::new();
    let rows = |r: &[(&str, &str, Option<&str>)]| -> String {
        let r: Vec<Value> = r
            .iter()
            .map(|(det, fp, id)| match id {
                Some(id) => json!({"detector": det, "fingerprint": fp, "id": id}),
                None => json!({"detector": det, "fingerprint": fp}),
            })
            .collect();
        json!({"payload": {"findings": r}}).to_string()
    };
    let base = rows(&[
        ("d55", "fa", Some("a1")),
        ("d55", "fa", Some("a2")),
        ("d55", "fb", Some("b1")),
        ("d55", "fz", Some("z1")),
        ("d55", "fz", Some("z2")),
        ("d55", "fq", None),
        ("d55", "fq", None),
        ("d55", "fd", Some("d1")),
    ]);
    let head = rows(&[
        ("d55", "fa", Some("a1")),
        ("d55", "fb", Some("b1")),
        ("d55", "fb", Some("b2")),
        ("d55", "fz1", Some("z1")),
        ("d55", "fz2", Some("z2")),
        ("d55", "fq", None),
        ("d55", "fq", None),
        ("d54", "fd", Some("d1")),
    ]);
    for (side, v) in [("A", &base), ("B", &head)] {
        std::fs::write(t.path(&format!("fix/{side}/analyze-primary.json")), v).unwrap();
        std::fs::write(t.path(&format!("fix/{side}/analyze-all.json")), v).unwrap();
    }
    let out = t.out("out");
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    let f = &t.report("out")["findings"]["analyze-primary"];
    assert_eq!(f["total"], json!([8, 8]), "{f}");
    assert_eq!(
        f["multiplicity_moved"],
        json!([
            ["d55", "fa", 2, 1],
            ["d55", "fb", 1, 2],
            ["d55", "fz", 2, 0]
        ]),
        "{f}"
    );
    assert_eq!(f["members_moved"], json!([]), "{f}");
    assert_eq!(f["members_unknown"], json!([["d55", "fq", 2, 2]]), "{f}");
    assert_eq!(f["detector_mismatch"], json!([["d54/d55", "fd"]]), "{f}");
    let md = std::fs::read_to_string(t.path("out/report.md")).unwrap();
    assert!(
        md.contains("**ids unknown** under 1 shared fingerprint(s)")
            && md.contains("**detector mismatch** under 1 fingerprint(s)"),
        "{md}"
    );

    // The same-set case alone (review r2, sol: above, `fz` and `fd` change the
    // set and the detector counts in the same run): same fingerprints, same
    // total, same detector count, only the counts per fingerprint move.
    let base = rows(&[
        ("d55", "fa", Some("a1")),
        ("d55", "fa", Some("a2")),
        ("d55", "fb", Some("b1")),
    ]);
    let head = rows(&[
        ("d55", "fa", Some("a1")),
        ("d55", "fb", Some("b1")),
        ("d55", "fb", Some("b2")),
    ]);
    for (side, v) in [("A", &base), ("B", &head)] {
        std::fs::write(t.path(&format!("fix/{side}/analyze-primary.json")), v).unwrap();
        std::fs::write(t.path(&format!("fix/{side}/analyze-all.json")), v).unwrap();
    }
    let out = t.out("same-set");
    let (code, text) = t.run(&["--out", &out, &t.c1, &t.c2], &[]);
    assert_eq!(code, Some(0), "{text}");
    let f = &t.report("same-set")["findings"]["analyze-primary"];
    assert_eq!(f["total"], json!([3, 3]), "{f}");
    assert_eq!(
        f["multiplicity_moved"],
        json!([["d55", "fa", 2, 1], ["d55", "fb", 1, 2]]),
        "the same-set count changes were missed: {f}"
    );
}

/// The measurement runs only inside the CDO slot: held by another, the run
/// waits, and gives up at `--slot-timeout` without measuring anything (the
/// builds come before the slot, so they are done).
///
/// Mutation (2026-11-15): `take_slot` accepting any exit (`if rc not in (0, 4)`
/// -> `if False`) -> the run measures under the other holder's slot and exits 0.
#[test]
fn before_after_measures_only_inside_the_slot() {
    let t = Ba::new();
    let mut hold = Hold::new(&t, "");
    let out = t.out("out");
    let (code, text) = t.run(&["--slot-timeout", "2", "--out", &out, &t.c1, &t.c2], &[]);
    hold.end();
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("the CDO slot was not taken"), "{text}");
    assert!(!t.log().contains("fingerprint"), "{}", t.log());
}

/// ⟨TLS-89⟩ A build still running when its timed hold is to begin: the run
/// measures nothing, says why, fails, and frees the slot it took. The stand-in
/// build is bash through a symlink named `t89b<test pid>` (unique: `pgrep -x`
/// sees the whole machine).
///
/// Mutation (2026-11-18): the `rc == 6` branch of `take_slot` dropped -> the
/// generic "the CDO slot was not taken" message instead (red); restored, green.
#[test]
fn before_after_measures_nothing_beside_a_build() {
    let t = Ba::new();
    let name = format!("t89b{}", std::process::id());
    let fake = t.path(&name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    // One process: the builtin `read` waits on a pipe nobody writes, so killing
    // it leaves no `sleep` child holding the test's output (a nextest leak).
    let build = crate::cdo_slot::Reap(
        crate::clean_env::command(&fake)
            .args(["-c", "read -t 600 _ || true"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = t.out("out");
    let (code, text) = t.run(
        &["--out", &out, &t.c1, &t.c2],
        &[
            ("CDO_SLOT_BUILD_NAMES", name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "1"),
        ],
    );
    drop(build);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("the timed hold was refused"), "{text}");
    assert!(!t.log().contains("fingerprint"), "{}", t.log());
    let status = crate::clean_env::command("bash")
        .arg(t.path("repo/scripts/cdo-slot"))
        .arg("status")
        .env("CDO_SLOT_DIR", t.path("slot"))
        .output()
        .unwrap();
    assert_eq!(
        status.status.code(),
        Some(0),
        "the slot stays held: {status:?}"
    );
}

/// Run inside a hold it shares (the holder's `CDO_SLOT_TOKEN`, as under
/// `cdo-slot run` or `land`), it measures and leaves that hold in place.
///
/// Mutation (2026-11-15): the release made unconditional (`if owned:` ->
/// `if True:`) -> the outer hold is gone after the run.
#[test]
fn before_after_keeps_a_hold_it_shares() {
    let t = Ba::new();
    let mut hold = Hold::new(&t, "tok-ba");
    let out = t.out("out");
    let (code, text) = t.run(
        &["--out", &out, &t.c1, &t.c2],
        &[("CDO_SLOT_TOKEN", "tok-ba")],
    );
    let status = crate::clean_env::command("bash")
        .arg(t.path("repo/scripts/cdo-slot"))
        .arg("status")
        .env("CDO_SLOT_DIR", t.path("slot"))
        .output()
        .unwrap();
    hold.end();
    assert_eq!(code, Some(0), "{text}");
    assert!(t.log().contains("slot: held by other"), "{}", t.log());
    assert_eq!(status.status.code(), Some(1), "{status:?}");
}

/// A slot hold by "other", bound to a sleeper; `token` ("" for none) is the
/// holder's token. The sleeper is killed at `end` or drop, which ends the hold.
struct Hold(std::process::Child);

impl Hold {
    fn new(t: &Ba, token: &str) -> Hold {
        let sleeper = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .unwrap();
        let mut cmd = crate::clean_env::command("bash");
        cmd.arg(t.path("repo/scripts/cdo-slot"))
            .args(["acquire", "other", "test hold", "--bind"])
            .arg(sleeper.id().to_string())
            .env("CDO_SLOT_DIR", t.path("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0");
        if !token.is_empty() {
            cmd.env("CDO_SLOT_TOKEN", token);
        }
        let held = cmd.output().unwrap();
        let hold = Hold(sleeper);
        assert!(held.status.success(), "{held:?}");
        hold
    }

    fn end(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.end();
    }
}
