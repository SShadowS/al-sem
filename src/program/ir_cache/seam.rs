//! The seam (spec Design 5): one dependency package's files parsed, or decoded from the IR
//! cache, inside the caller's `par_iter`; on a miss, encoded, round-tripped and published.

use super::codec::{decode_file, encode_file, round_trip_ok};
use super::counters::Counters;
use super::envelope::{Diag, MAX_ENTRY, MAX_FILE, Slot, open_envelope, write_envelope};
use super::key::IrKey;
use super::{IrCache, Warn};
use crate::program::dep_summary::LiveTrees;
use crate::snapshot::cache::{CacheDir, LockKind, TextDigest, check_entry, text_entry_name};
use crate::snapshot::embedded::SourceFile;
use crate::snapshot::{AppUnit, ParsedFile};
use rayon::prelude::*;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Packages with fewer files are parsed, never cached (spec Design 6; step-1 table).
pub(crate) const SMALL_PACKAGE_MIN_FILES: usize = 8;

/// The soft limit on one key folder: past it, this process publishes nothing more.
const KEY_FOLDER_LIMIT: u64 = 4 << 30;

/// A tree was just made (parsed or decoded): the bound test's counter (`LiveTrees`), counted
/// from the tree's creation; the caller's `f` counts its drop.
fn born(observe: Option<&LiveTrees>) {
    if let Some(o) = observe {
        let now = o.live.fetch_add(1, SeqCst) + 1;
        o.peak.fetch_max(now, SeqCst);
    }
}

/// [`crate::snapshot::parse::parse_file`], counted by [`born`].
fn parse_one(unit: &AppUnit, sf: &SourceFile, observe: Option<&LiveTrees>) -> ParsedFile {
    let pf = crate::snapshot::parse::parse_file(unit, sf);
    born(observe);
    pf
}

/// `unit`'s files as `f` sees them, in order: decoded from the cache entry of its text digest
/// when there is a valid one, else parsed (and, on a miss, published). `None`: today's parse.
/// Call it on [`crate::big_stack::big_stack_pool`]; `f` runs per file inside the `par_iter`,
/// so a caller that drops each tree in `f` keeps one tree per worker thread. `observe` counts
/// each tree as it is made (the caller counts its drop).
pub(crate) fn parse_package<R: Send>(
    unit: &AppUnit,
    cache: Option<&IrCache>,
    observe: Option<&LiveTrees>,
    f: impl Fn(ParsedFile) -> R + Sync,
) -> Vec<R> {
    let Some(source) = unit.source.as_ref() else {
        return Vec::new();
    };
    let files = &source.files;
    let parse_all = || -> Vec<R> {
        files
            .par_iter()
            .map(|sf| f(parse_one(unit, sf, observe)))
            .collect()
    };
    let Some(c) = cache else {
        return parse_all();
    };
    let n = files.len();
    Counters::add(&c.counters.seam_files, n);
    // Hashed once per process, at its first seam use (spec "The key").
    let key = if c.enabled { c.key() } else { None };
    c.first_use(key.as_ref());
    let (Some(digest), Some(key)) = (source.text_digest, key) else {
        Counters::add(&c.counters.skipped_off, n);
        return parse_all();
    };
    if n < SMALL_PACKAGE_MIN_FILES {
        Counters::add(&c.counters.skipped_small, n);
        return parse_all();
    }
    let ir = match c.open_ir(false) {
        IrDir::Unusable => {
            c.warn_once(
                Warn::Unwritable,
                &format!(
                    "IR cache {} is not a private directory of this user: off",
                    c.ir_dir.display()
                ),
            );
            Counters::add(&c.counters.skipped_off, n);
            return parse_all();
        }
        ir => ir,
    };
    Counters::add(&c.counters.eligible_files, n);
    let name = entry_name(&digest);
    // A miss charges `parsed_miss`; an entry that is there but refused (unreadable, or its
    // envelope fails a check) charges `refused`, so a gate sees the repair (sol r1).
    let mut charge = &c.counters.parsed_miss;
    let stored = match &ir {
        IrDir::Usable(d) => d.subdir(&key.hex(), false).and_then(|kd| {
            kd.read_owned(&name, MAX_ENTRY).unwrap_or_else(|e| {
                log::warn!("IR cache entry {name} unreadable ({e:#}): refilled");
                charge = &c.counters.refused;
                None
            })
        }),
        _ => None,
    };
    if let Some(bytes) = stored {
        match open_envelope(&bytes, &key, &digest, n) {
            Ok(slots) => {
                let refused = AtomicBool::new(false);
                let out = hit(c, unit, files, &slots, &key, &refused, observe, &f);
                c.touch(&key);
                if refused.load(Ordering::Relaxed) {
                    // A defect (spec "Failure cases"): refill, rewriting only changed bytes.
                    if let Some(slots) = fresh_slots(c, unit, &digest) {
                        publish(c, &key, &digest, &slots, Some(&bytes));
                    }
                }
                return out;
            }
            // Refilled below; `publish` replaces the refused entry under the lock.
            Err(e) => {
                log::warn!(
                    "IR cache entry {name} of {} refused ({e:#}): refilled",
                    unit.id.name
                );
                charge = &c.counters.refused;
            }
        }
    }
    fill(c, unit, &key, &digest, charge, observe, &f)
}

/// Decode each stored file (a hit), parse the others.
#[allow(clippy::too_many_arguments)] // one call site; the arguments are the hit's inputs
fn hit<R: Send>(
    c: &IrCache,
    unit: &AppUnit,
    files: &[SourceFile],
    slots: &[Option<&[u8]>],
    key: &IrKey,
    refused: &AtomicBool,
    observe: Option<&LiveTrees>,
    f: &(impl Fn(ParsedFile) -> R + Sync),
) -> Vec<R> {
    files
        .par_iter()
        .zip(slots.par_iter())
        .map(|(sf, slot)| {
            let Some(bytes) = slot else {
                Counters::add(&c.counters.skipped_unstored, 1);
                return f(parse_one(unit, sf, observe));
            };
            let decoded = decode_file(bytes);
            #[cfg(test)]
            let decoded = super::codec::test_hook::on_hit(decoded);
            match decoded {
                Ok(af) => {
                    born(observe);
                    if c.verify {
                        let fresh = al_syntax::parse(&sf.text);
                        assert!(
                            fresh == af,
                            "IR cache VERIFY: decoded IR of {} {} differs from a fresh parse \
                             (key {})",
                            unit.id.name,
                            sf.virtual_path,
                            key.hex()
                        );
                        Counters::add(&c.counters.verified_hits, 1);
                    }
                    Counters::add(&c.counters.disk_hits, 1);
                    f(ParsedFile {
                        virtual_path: sf.virtual_path.clone(),
                        file: Arc::new(af),
                        provenance: unit.provenance.clone(),
                        text: sf.text.clone(),
                    })
                }
                Err(e) => {
                    log::warn!(
                        "IR cache: {} {} failed to decode under a valid hash ({e:#}, key {}): \
                         parsed",
                        unit.id.name,
                        sf.virtual_path,
                        key.hex()
                    );
                    Counters::add(&c.counters.refused, 1);
                    refused.store(true, Ordering::Relaxed);
                    f(parse_one(unit, sf, observe))
                }
            }
        })
        .collect()
}

/// A miss (or a refused entry): parse from an owned, checked copy of the ENG-113 entry and
/// publish under its digest; when that entry no longer carries the digest, parse the mapped
/// text as today and publish nothing (spec "The text").
fn fill<R: Send>(
    c: &IrCache,
    unit: &AppUnit,
    key: &IrKey,
    digest: &TextDigest,
    charge: &AtomicU64,
    observe: Option<&LiveTrees>,
    f: &(impl Fn(ParsedFile) -> R + Sync),
) -> Vec<R> {
    let files = &unit.source.as_ref().expect("a package with source").files;
    Counters::add(charge, files.len());
    let owned = match owned_texts(c, unit, digest) {
        Ok(owned) => owned,
        Err(skip) => {
            let (counter, why) = match skip {
                FillSkip::Changed => (
                    &c.counters.fill_text_changed,
                    "changed since it was mapped".to_string(),
                ),
                FillSkip::Unreadable(e) => (
                    &c.counters.fill_text_unreadable,
                    format!("cannot be read ({e})"),
                ),
            };
            Counters::add(counter, 1);
            log::warn!(
                "IR cache: the text cache entry of {} {why}: parsed, nothing published",
                unit.id.name
            );
            return files
                .par_iter()
                .map(|sf| f(parse_one(unit, sf, observe)))
                .collect();
        }
    };
    let (out, slots): (Vec<R>, Vec<Slot>) = files
        .par_iter()
        .zip(owned.ranges.par_iter())
        .map(|(sf, r)| {
            let af = al_syntax::parse(owned.text(r));
            born(observe);
            let slot = encode_slot(c, unit, sf, &af);
            let pf = ParsedFile {
                virtual_path: sf.virtual_path.clone(),
                file: Arc::new(af),
                provenance: unit.provenance.clone(),
                text: sf.text.clone(),
            };
            (f(pf), slot)
        })
        .unzip();
    drop(owned);
    publish(c, key, digest, &slots, None);
    out
}

/// A refill's slots: every file parsed again from the owned text (no consumer sees them).
fn fresh_slots(c: &IrCache, unit: &AppUnit, digest: &TextDigest) -> Option<Vec<Slot>> {
    let files = &unit.source.as_ref()?.files;
    let owned = owned_texts(c, unit, digest).ok()?;
    Some(
        files
            .par_iter()
            .zip(owned.ranges.par_iter())
            .map(|(sf, r)| encode_slot(c, unit, sf, &al_syntax::parse(owned.text(r))))
            .collect(),
    )
}

/// The publish gate (spec Design 3): a file is stored only when its encoding round-trips and
/// fits [`MAX_FILE`].
fn encode_slot(c: &IrCache, unit: &AppUnit, sf: &SourceFile, af: &al_syntax::ir::AlFile) -> Slot {
    let enc = encode_file(af);
    if enc.len() as u64 > MAX_FILE {
        return Slot::NotStored;
    }
    if !round_trip_ok(af, &enc) {
        Counters::add(&c.counters.roundtrip_failed, 1);
        c.warn_once(
            Warn::RoundTrip,
            &format!(
                "IR cache: the IR of {} {} does not survive its round trip (a codec defect): \
                 not stored",
                unit.id.name, sf.virtual_path
            ),
        );
        return Slot::NotStored;
    }
    Slot::Stored(enc)
}

/// An owned copy of a package's ENG-113 entry and each file's range in it.
struct Owned {
    bytes: Vec<u8>,
    ranges: Vec<Range<usize>>,
}

impl Owned {
    fn text(&self, r: &Range<usize>) -> &str {
        // Checked UTF-8 in `owned_texts`.
        std::str::from_utf8(&self.bytes[r.clone()]).expect("checked UTF-8")
    }
}

/// Why a fill parses the mapped text and publishes nothing.
enum FillSkip {
    /// The entry no longer carries the digest, or no longer lists the unit's files.
    Changed,
    /// The text cache directory or entry cannot be read (not private, gone, over the cap).
    Unreadable(String),
}

/// The ENG-113 entry of `unit`, read owned and checked by `check_entry`, when it still
/// carries `digest` and lists exactly the unit's files in order.
fn owned_texts(c: &IrCache, unit: &AppUnit, digest: &TextDigest) -> Result<Owned, FillSkip> {
    let source = unit.source.as_ref().ok_or(FillSkip::Changed)?;
    let d = CacheDir::open(c.text_dir.as_path()).ok_or_else(|| {
        FillSkip::Unreadable(format!(
            "{} is not a private directory",
            c.text_dir.display()
        ))
    })?;
    let bytes = match d.read_owned(&text_entry_name(&source.content_hash), MAX_ENTRY) {
        Ok(Some(b)) => b,
        Ok(None) => return Err(FillSkip::Unreadable("the entry is gone".into())),
        Err(e) => return Err(FillSkip::Unreadable(format!("{e:#}"))),
    };
    let (got, index) = check_entry(&bytes, &source.content_hash).map_err(|_| FillSkip::Changed)?;
    if &got != digest || index.len() != source.files.len() {
        return Err(FillSkip::Changed);
    }
    let mut ranges = Vec::with_capacity(index.len());
    for ((path, r), sf) in index.into_iter().zip(source.files.iter()) {
        if path != sf.virtual_path || std::str::from_utf8(&bytes[r.clone()]).is_err() {
            return Err(FillSkip::Changed);
        }
        ranges.push(r);
    }
    Ok(Owned { bytes, ranges })
}

/// The lock of key folder `<key hex>`: a file in `ir/` beside the folder, never inside it and
/// never removed, so removing a folder never unlinks a held lock (Fable r1, sol r1).
/// Publishers and `last-used` writers hold it shared, the prune exclusive.
pub(crate) fn key_lock_name(key_hex: &str) -> String {
    format!("{key_hex}.lock")
}

/// Publish `slots` as the entry of `digest` (spec Design 9): the key folder's lock (in `ir/`)
/// shared and the entry's lock, both without waiting (busy: another process publishes or
/// prunes, skip). Under the locks the entry is read again: a valid one is kept (another
/// process published it), unless this is a refill (`old`, the stored bytes a decode refused)
/// whose new bytes differ. The envelope streams into the temp file (one package's slots and
/// the writer's buffer in memory, not another whole copy).
fn publish(c: &IrCache, key: &IrKey, digest: &TextDigest, slots: &[Slot], old: Option<&[u8]>) {
    let IrDir::Usable(ir) = c.open_ir(true) else {
        c.warn_once(
            Warn::Unwritable,
            &format!(
                "IR cache {} cannot be made a private directory: nothing published",
                c.ir_dir.display()
            ),
        );
        return;
    };
    let busy = || Counters::add(&c.counters.publish_skipped_busy, 1);
    let key_hex = key.hex();
    let Some(_key_lock) = ir.lock(&key_lock_name(&key_hex), LockKind::SharedNow) else {
        return busy();
    };
    let Some(kd) = ir.subdir(&key_hex, true) else {
        c.warn_once(
            Warn::Unwritable,
            &format!("IR cache key folder under {} unusable", c.ir_dir.display()),
        );
        return;
    };
    let name = entry_name(digest);
    let Some(_entry) = kd.lock(&format!("{}.lock", hex(digest)), LockKind::ExclusiveNow) else {
        return busy();
    };
    let diag = Diag {
        grammar_commit: al_syntax::language::GRAMMAR_COMMIT,
        version: env!("CARGO_PKG_VERSION"),
    };
    // A refill's new bytes, built only on that (defect) path to compare with the old ones.
    let mut refill_bytes = None;
    let replace = match kd.read_owned(&name, MAX_ENTRY) {
        Ok(Some(theirs)) if open_envelope(&theirs, key, digest, slots.len()).is_ok() => {
            // A valid entry: another process published it, or (a refill) the one we refused.
            let Some(old) = old else { return };
            if old != theirs.as_slice() {
                return;
            }
            let mut buf = Vec::new();
            if write_envelope(&mut buf, key, digest, &diag, slots).is_err() || buf == theirs {
                return;
            }
            refill_bytes = Some(buf);
            true
        }
        Ok(None) => false,
        _ => true,
    };
    if folder_bytes(&kd) > KEY_FOLDER_LIMIT {
        c.warn_once(
            Warn::FolderFull,
            &format!(
                "IR cache key folder {} passed {KEY_FOLDER_LIMIT} bytes: nothing more published",
                kd.path().display()
            ),
        );
        return;
    }
    let written = kd.write(&name, replace, |out| match &refill_bytes {
        Some(buf) => Ok(out.write_all(buf)?),
        None => write_envelope(out, key, digest, &diag, slots),
    });
    match written {
        Ok(()) => {
            Counters::add(&c.counters.published_packages, 1);
            c.touch(key);
        }
        Err(e) => log::warn!("IR cache entry {name} not published ({e:#})"),
    }
}

/// The bytes of a key folder's files (best effort).
fn folder_bytes(kd: &CacheDir<'_>) -> u64 {
    std::fs::read_dir(kd.path()).map_or(0, |rd| {
        rd.filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    })
}

pub(crate) fn hex(b: &[u8; 32]) -> String {
    blake3::Hash::from_bytes(*b).to_hex().to_string()
}

pub(crate) fn entry_name(digest: &TextDigest) -> String {
    format!("{}.bin", hex(digest))
}

/// The `ir/` folder as a package sees it.
pub(crate) enum IrDir {
    Usable(CacheDir<'static>),
    /// Not there yet: a miss (a writer makes it).
    Missing,
    /// There, but not a private directory of this user: the cache is off.
    Unusable,
}

impl IrCache {
    /// The `ir/` folder; with `create` (a writer), made owner-only first when missing.
    pub(crate) fn open_ir(&self, create: bool) -> IrDir {
        if create {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                let _ = std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&self.ir_dir);
            }
        }
        match std::fs::symlink_metadata(&self.ir_dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => IrDir::Missing,
            Err(_) => IrDir::Unusable,
            Ok(_) => match CacheDir::open(self.ir_dir.clone()) {
                Some(d) => IrDir::Usable(d),
                None => IrDir::Unusable,
            },
        }
    }

    /// A hit or a publish under `key`: `last-used` (forced at this process's first use, else
    /// when a day old) and the throttled prune (spec Design 9). The key folder is opened again
    /// each time, so a pruned one is not written through a stale handle.
    pub(crate) fn touch(&self, key: &IrKey) {
        #[cfg(unix)]
        {
            let IrDir::Usable(ir) = self.open_ir(false) else {
                return;
            };
            let now = std::time::SystemTime::now();
            let key_hex = key.hex();
            // Under the key folder's lock (shared): a prune holding it exclusive is removing
            // the folder, and this `last-used` would only land in a folder going away.
            if let Some(_held) = ir.lock(&key_lock_name(&key_hex), LockKind::SharedNow)
                && let Some(kd) = ir.subdir(&key_hex, false)
            {
                let first = !self.touched.swap(true, Ordering::Relaxed);
                super::prune::touch_last_used(&kd, first, now);
            }
            super::prune::maybe_prune(&ir, key, now);
        }
        #[cfg(not(unix))]
        let _ = key;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::app_image::AppImage;
    use crate::engine::deps::app_package_zip::test_apps::build_app;
    use crate::program::dep_summary::tests::{app_id, unit};
    use crate::snapshot::TrustTier;
    use crate::snapshot::provider::SourceRoot;
    use std::path::Path;
    use std::sync::atomic::Ordering::SeqCst;

    const KEY: IrKey = IrKey([3; 32]);

    fn text(i: usize) -> String {
        format!(
            "codeunit {} \"C{i}\"\n{{\n    procedure P{i}()\n    var\n        Ø: Integer;\n    begin\n#if A\n        Ø := {i};\n#endif\n        Q();\n    end;\n\n    procedure Q()\n    begin\n    end;\n}}\n",
            70000 + i
        )
    }

    /// A dependency unit of `texts`, its source mapped from a real ENG-113 entry in
    /// `text_dir` (so it carries a text digest), like `EmbeddedAppProvider` makes it.
    fn package(text_dir: &Path, texts: &[String]) -> AppUnit {
        let entries: Vec<(String, &[u8])> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| (format!("src/F{i:02}.al"), t.as_bytes()))
            .collect();
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        let img = AppImage::from_bytes("pkg.app", build_app(&refs));
        let (files, hash, digest) =
            crate::snapshot::cache::cached_source_in(text_dir, &img).unwrap();
        assert!(digest.is_some(), "precondition: a mapped entry");
        let mut u = unit(&app_id("Dep"), TrustTier::EmbeddedSource, &[]);
        u.source = Some(SourceRoot {
            files: Arc::new(files),
            tier: TrustTier::EmbeddedSource,
            content_hash: hash,
            text_digest: digest,
        });
        u
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(text).collect()
    }

    type Out = Vec<(String, Arc<al_syntax::ir::AlFile>)>;

    fn run(u: &AppUnit, c: Option<&IrCache>) -> Out {
        crate::big_stack::big_stack_pool()
            .install(|| parse_package(u, c, None, |pf| (pf.virtual_path, pf.file)))
    }

    fn count(c: &IrCache, f: impl Fn(&Counters) -> &std::sync::atomic::AtomicU64) -> u64 {
        f(&c.counters).load(SeqCst)
    }

    struct Dirs {
        _tmp: tempfile::TempDir,
        text: std::path::PathBuf,
        ir: std::path::PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().unwrap();
        let (text, ir) = (tmp.path().join("text"), tmp.path().join("ir"));
        std::fs::create_dir(&text).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&text, std::fs::Permissions::from_mode(0o700)).unwrap();
        Dirs {
            _tmp: tmp,
            text,
            ir,
        }
    }

    fn cache(d: &Dirs, verify: bool) -> IrCache {
        IrCache::with_key(d.ir.clone(), d.text.clone(), verify, KEY)
    }

    fn entry_path(d: &Dirs, u: &AppUnit) -> std::path::PathBuf {
        let digest = u.source.as_ref().unwrap().text_digest.unwrap();
        d.ir.join(KEY.hex()).join(entry_name(&digest))
    }

    const N: usize = SMALL_PACKAGE_MIN_FILES + 2;

    #[test]
    fn cold_then_warm_equal_cache_off() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        let cold = cache(&d, false);
        assert_eq!(run(&u, Some(&cold)), off, "cold");
        assert_eq!(count(&cold, |c| &c.parsed_miss), N as u64);
        assert_eq!(count(&cold, |c| &c.published_packages), 1);
        assert_eq!(count(&cold, |c| &c.eligible_files), N as u64);
        assert!(entry_path(&d, &u).is_file());
        let warm = cache(&d, false);
        assert_eq!(run(&u, Some(&warm)), off, "warm");
        assert_eq!(count(&warm, |c| &c.disk_hits), N as u64);
        assert_eq!(count(&warm, |c| &c.published_packages), 0);
        assert!(cold.counters.balanced() && warm.counters.balanced());
    }

    #[test]
    fn refused_entry_is_refilled_and_next_run_hits() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        run(&u, Some(&cache(&d, false)));
        let path = entry_path(&d, &u);
        let mut b = std::fs::read(&path).unwrap();
        let at = b.len() - 40;
        b[at] ^= 1;
        std::fs::write(&path, &b).unwrap();
        let c2 = cache(&d, false);
        assert_eq!(run(&u, Some(&c2)), off);
        // sol r1: a refused envelope counts as refused (a gate fails on it), not as a miss.
        assert_eq!(count(&c2, |c| &c.refused), N as u64);
        assert_eq!(count(&c2, |c| &c.parsed_miss), 0);
        assert_eq!(count(&c2, |c| &c.published_packages), 1, "replaced");
        assert!(c2.counters.balanced());
        let c3 = cache(&d, false);
        assert_eq!(run(&u, Some(&c3)), off);
        assert_eq!(count(&c3, |c| &c.disk_hits), N as u64);
    }

    /// A file whose IR does not survive its round trip (the codec hook moves a kind id) is not
    /// stored: counted, parsed on every hit, results unchanged.
    #[test]
    fn round_trip_failure_is_not_stored() {
        let d = dirs();
        let mut t = texts(N);
        t[1] = format!(
            "codeunit 70100 \"{}\"\n{{\n    procedure P()\n    begin\n    end;\n}}\n",
            super::super::codec::test_hook::ALIAS_ON_DECODE
        );
        let u = package(&d.text, &t);
        let off = run(&u, None);
        let cold = cache(&d, false);
        assert_eq!(run(&u, Some(&cold)), off);
        assert_eq!(count(&cold, |c| &c.roundtrip_failed), 1);
        assert_eq!(count(&cold, |c| &c.published_packages), 1);
        let warm = cache(&d, false);
        assert_eq!(run(&u, Some(&warm)), off);
        assert_eq!(count(&warm, |c| &c.disk_hits), N as u64 - 1);
        assert_eq!(count(&warm, |c| &c.skipped_unstored), 1);
        assert!(warm.counters.balanced());
    }

    /// The ENG-113 entry replaced (another valid text of the same package) between the map
    /// and the fill: parsed from the map, nothing published.
    #[test]
    fn changed_text_entry_publishes_nothing() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        let source = u.source.as_ref().unwrap();
        let mut other: Vec<_> = source.files.iter().cloned().collect();
        other[0].text = text(99).into();
        let td = CacheDir::open(d.text.as_path()).unwrap();
        td.write(&text_entry_name(&source.content_hash), true, |o| {
            crate::snapshot::cache::write_entry(o, &source.content_hash, &other)
        })
        .unwrap();
        let c = cache(&d, false);
        assert_eq!(run(&u, Some(&c)), off);
        assert_eq!(count(&c, |c| &c.fill_text_changed), 1);
        assert_eq!(count(&c, |c| &c.published_packages), 0);
        assert!(!entry_path(&d, &u).exists(), "nothing published");
    }

    /// VERIFY: a stored file that decodes to IR other than a fresh parse fails the process,
    /// naming the file.
    #[test]
    fn verify_mismatch_panics() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        run(&u, Some(&cache(&d, false)));
        let source = u.source.as_ref().unwrap();
        let mut slots: Vec<Slot> = source
            .files
            .iter()
            .map(|f| Slot::Stored(encode_file(&al_syntax::parse(&f.text))))
            .collect();
        slots[2] = Slot::Stored(encode_file(&al_syntax::parse(&text(98))));
        let mut b = Vec::new();
        let diag = Diag {
            grammar_commit: "t",
            version: "t",
        };
        write_envelope(&mut b, &KEY, &source.text_digest.unwrap(), &diag, &slots).unwrap();
        std::fs::write(entry_path(&d, &u), b).unwrap();
        let ok = cache(&d, false);
        run(&u, Some(&ok));
        assert_eq!(
            count(&ok, |c| &c.disk_hits),
            N as u64,
            "precondition: decodes"
        );
        let v = cache(&d, true);
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&u, Some(&v))))
            .expect_err("VERIFY fails the process");
        let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("src/F02.al"), "{msg}");
        // A matching hit is verified and counted.
        run(&u, Some(&cache(&d, false)));
        let d2 = dirs();
        let u2 = package(&d2.text, &texts(N));
        run(&u2, Some(&cache(&d2, false)));
        let v2 = cache(&d2, true);
        run(&u2, Some(&v2));
        assert_eq!(count(&v2, |c| &c.verified_hits), N as u64);
    }

    /// Review Focus 3: another process publishing the same entry holds its lock: this one
    /// skips (no wait), counts it, and its results are unchanged.
    #[test]
    fn busy_entry_lock_skips_publish() {
        use std::os::unix::fs::DirBuilderExt;
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        let kd = d.ir.join(KEY.hex());
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&kd)
            .unwrap();
        let digest = u.source.as_ref().unwrap().text_digest.unwrap();
        let held = CacheDir::open(kd.as_path())
            .unwrap()
            .lock(&format!("{}.lock", hex(&digest)), LockKind::Exclusive)
            .unwrap();
        let c = cache(&d, false);
        let (tx, rx) = std::sync::mpsc::channel();
        let got = std::thread::scope(|s| {
            s.spawn(|| {
                let out = run(&u, Some(&c));
                tx.send((out, count(&c, |c| &c.publish_skipped_busy)))
                    .unwrap();
            });
            let got = rx.recv_timeout(std::time::Duration::from_secs(30));
            // Released either way, so a publish that waits ends and the test fails, not hangs.
            drop(held);
            got
        });
        let (out, busy) = got.expect("the publish waited for the entry lock");
        assert_eq!(out, off);
        assert_eq!(busy, 1);
        assert!(!entry_path(&d, &u).exists());
    }

    /// Review Focus 2: an `ir/` folder other users can write: the analysis runs as with the
    /// cache off, every file `skipped_off`, nothing written.
    #[test]
    fn unusable_root_runs_with_cache_off() {
        use std::os::unix::fs::PermissionsExt;
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        std::fs::create_dir(&d.ir).unwrap();
        std::fs::set_permissions(&d.ir, std::fs::Permissions::from_mode(0o775)).unwrap();
        let c = cache(&d, false);
        assert_eq!(run(&u, Some(&c)), off);
        assert_eq!(count(&c, |c| &c.skipped_off), N as u64);
        assert_eq!(count(&c, |c| &c.seam_files), N as u64);
        assert_eq!(std::fs::read_dir(&d.ir).unwrap().count(), 0);
    }

    /// Review Focus 5: a package below the threshold is parsed and never cached; a
    /// symbol-only one (no source) yields nothing and counts nothing.
    #[test]
    fn small_and_symbol_only_packages_are_skipped() {
        let d = dirs();
        let small = package(&d.text, &texts(SMALL_PACKAGE_MIN_FILES - 1));
        let off = run(&small, None);
        let c = cache(&d, false);
        assert_eq!(run(&small, Some(&c)), off);
        assert_eq!(
            count(&c, |c| &c.skipped_small),
            SMALL_PACKAGE_MIN_FILES as u64 - 1
        );
        assert_eq!(count(&c, |c| &c.eligible_files), 0);
        assert!(!d.ir.exists(), "nothing written");
        let mut symbols = unit(&app_id("Sym"), TrustTier::SymbolOnly, &[]);
        symbols.source = None;
        assert!(run(&symbols, Some(&c)).is_empty());
        assert!(c.counters.balanced());
    }

    /// A key folder pruned while a process runs is made again by its next publish (no handle
    /// to it is kept between packages).
    #[test]
    fn pruned_key_folder_is_recreated() {
        let d = dirs();
        let a = package(&d.text, &texts(N));
        let b = package(&d.text, &(10..10 + N).map(text).collect::<Vec<_>>());
        let c = cache(&d, false);
        run(&a, Some(&c));
        assert!(entry_path(&d, &a).is_file());
        std::fs::remove_dir_all(d.ir.join(KEY.hex())).unwrap();
        run(&b, Some(&c));
        assert!(entry_path(&d, &b).is_file(), "published into a new folder");
        assert_eq!(count(&c, |c| &c.published_packages), 2);
    }

    /// The trees alive at once, counted from each tree's creation (decode or parse, inside
    /// `parse_package`) to its drop (in `f`), stay within one per worker thread on a warm
    /// hit, as on a parse (`dep_summary`'s `LiveTrees` bound). Counting from creation is what
    /// lets it fail: a seam that decoded every tree before calling `f` would show `n`.
    #[test]
    fn live_trees_bound_holds_on_hits() {
        let d = dirs();
        let n = 64;
        let u = package(&d.text, &texts(n));
        run(&u, Some(&cache(&d, false)));
        let warm = cache(&d, false);
        let live = LiveTrees::default();
        let threads = crate::big_stack::big_stack_pool().current_num_threads();
        crate::big_stack::big_stack_pool().install(|| {
            parse_package(&u, Some(&warm), Some(&live), |pf| {
                drop(pf);
                live.live.fetch_sub(1, SeqCst);
            })
        });
        assert_eq!(
            count(&warm, |c| &c.disk_hits),
            n as u64,
            "precondition: hits"
        );
        assert_eq!(
            live.live.load(SeqCst),
            0,
            "every tree was counted and dropped"
        );
        let peak = live.peak.load(SeqCst);
        assert!(
            peak >= 1 && peak <= threads,
            "{peak} trees, {threads} workers"
        );
    }

    /// The child half of [`batch_state_disables_installed_cache`] (process-wide state: the
    /// installed cache, the batch folder, a `Suspend`).
    #[test]
    #[ignore = "run by batch_state_disables_installed_cache as a child"]
    fn batch_child_mode() {
        if std::env::var_os("ALSEM_IR_BATCH_CHILD").is_none() {
            return;
        }
        let _report = super::super::install_from_env();
        assert!(super::super::installed().is_some(), "installed");
        // sol r1: installing makes no directory (the text cache root nor `ir/`).
        let xdg = std::env::var_os("XDG_CACHE_HOME").expect("set by the parent");
        assert!(
            !std::path::Path::new(&xdg).exists(),
            "install made the cache root"
        );
        let ir = std::env::var_os("ALSEM_IR_CACHE_DIR").expect("set by the parent");
        assert!(!std::path::Path::new(&ir).exists(), "install made ir/");
        {
            let _s = super::super::Suspend::new();
            assert!(super::super::installed().is_none(), "suspended");
        }
        assert!(super::super::installed().is_some(), "resumed");
        crate::engine::batch::claim::set_dir(std::env::temp_dir());
        assert!(super::super::installed().is_none(), "batch: off");
        println!("BATCH-CHILD-OK");
    }

    #[test]
    fn batch_state_disables_installed_cache() {
        let missing = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "program::ir_cache::seam::tests::batch_child_mode",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ALSEM_IR_BATCH_CHILD", "1")
            .env("ALSEM_IR_CACHE_DIR", missing.path().join("ir"))
            .env("XDG_CACHE_HOME", missing.path().join("xdg"))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("BATCH-CHILD-OK"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn entries_under(ir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for kd in std::fs::read_dir(ir).into_iter().flatten().flatten() {
            for e in std::fs::read_dir(kd.path()).into_iter().flatten().flatten() {
                if e.path().extension().is_some_and(|x| x == "bin") {
                    out.push(e.path());
                }
            }
        }
        out.sort();
        out
    }

    /// Spec "The key" test: two keys on one root keep two entries, and no hit crosses.
    #[test]
    fn two_keys_one_root_never_cross() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        let a = cache(&d, false);
        assert_eq!(run(&u, Some(&a)), off);
        let b = IrCache::with_key(d.ir.clone(), d.text.clone(), false, IrKey([4; 32]));
        assert_eq!(run(&u, Some(&b)), off);
        assert_eq!(count(&b, |c| &c.disk_hits), 0, "no hit across keys");
        assert_eq!(count(&b, |c| &c.published_packages), 1);
        assert_eq!(entries_under(&d.ir).len(), 2, "one entry per key");
        let a2 = cache(&d, false);
        assert_eq!(run(&u, Some(&a2)), off);
        assert_eq!(
            count(&a2, |c| &c.disk_hits),
            N as u64,
            "each key hits its own"
        );
    }

    /// A stored file that fails to decode under a valid hash (a defect, the codec hook here):
    /// refused, parsed, and the refill does not rewrite identical bytes.
    #[test]
    fn decode_failure_is_refused_and_identical_bytes_are_not_rewritten() {
        use std::os::unix::fs::MetadataExt;
        let d = dirs();
        let mut t = texts(N);
        t[1] = format!(
            "codeunit 70101 \"{}\"\n{{\n    procedure P()\n    begin\n    end;\n}}\n",
            super::super::codec::test_hook::FAIL_ON_HIT
        );
        let u = package(&d.text, &t);
        let off = run(&u, None);
        run(&u, Some(&cache(&d, false)));
        let path = entry_path(&d, &u);
        let before = std::fs::metadata(&path).unwrap();
        let c = cache(&d, false);
        assert_eq!(run(&u, Some(&c)), off);
        assert_eq!(count(&c, |c| &c.refused), 1);
        assert_eq!(count(&c, |c| &c.disk_hits), N as u64 - 1);
        assert_eq!(count(&c, |c| &c.published_packages), 0, "nothing rewritten");
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(
            (before.ino(), before.mtime_nsec()),
            (after.ino(), after.mtime_nsec()),
            "the entry is the same file"
        );
        assert!(c.counters.balanced());
    }

    /// Another process published the entry between this one's miss and its publish: under
    /// the locks the valid entry is read again and kept.
    #[test]
    fn an_entry_published_meanwhile_is_kept() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let digest = u.source.as_ref().unwrap().text_digest.unwrap();
        // Theirs: valid for this key, digest and file count, every file not stored.
        let theirs_slots: Vec<Slot> = (0..N).map(|_| Slot::NotStored).collect();
        let mut theirs = Vec::new();
        let diag = Diag {
            grammar_commit: "t",
            version: "t",
        };
        write_envelope(&mut theirs, &KEY, &digest, &diag, &theirs_slots).unwrap();
        let path = entry_path(&d, &u);
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path.parent().unwrap())
                .unwrap();
        }
        std::fs::write(&path, &theirs).unwrap();
        let c = cache(&d, false);
        let mine: Vec<Slot> = u
            .source
            .as_ref()
            .unwrap()
            .files
            .iter()
            .map(|f| Slot::Stored(encode_file(&al_syntax::parse(&f.text))))
            .collect();
        publish(&c, &KEY, &digest, &mine, None);
        assert_eq!(std::fs::read(&path).unwrap(), theirs, "theirs kept");
        assert_eq!(count(&c, |c| &c.published_packages), 0);
    }

    /// A key folder past [`KEY_FOLDER_LIMIT`] takes no more entries (warned once).
    #[test]
    fn a_full_key_folder_publishes_nothing() {
        use std::os::unix::fs::DirBuilderExt;
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        let kd = d.ir.join(KEY.hex());
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&kd)
            .unwrap();
        // Sparse: counts its length, holds no bytes on disk.
        std::fs::File::create(kd.join(format!("{}.bin", "b".repeat(64))))
            .unwrap()
            .set_len(KEY_FOLDER_LIMIT + 1)
            .unwrap();
        let c = cache(&d, false);
        assert_eq!(run(&u, Some(&c)), off);
        assert_eq!(count(&c, |c| &c.published_packages), 0);
        assert!(!entry_path(&d, &u).exists());
    }

    /// sol r1: damaged stamps (numbers past what `SystemTime` holds) on a real hit's
    /// maintenance path never panic: they read as absent.
    #[test]
    fn damaged_stamps_do_not_break_a_hit() {
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        run(&u, Some(&cache(&d, false)));
        std::fs::write(d.ir.join(KEY.hex()).join("last-used"), u64::MAX.to_string()).unwrap();
        std::fs::write(d.ir.join("last-prune"), u64::MAX.to_string()).unwrap();
        let c = cache(&d, false);
        assert_eq!(run(&u, Some(&c)), off);
        assert_eq!(count(&c, |c| &c.disk_hits), N as u64);
    }

    /// The text cache directory unusable at fill time: its own counter and message, not
    /// "changed since mapped" (Fable r1).
    #[test]
    fn an_unreadable_text_cache_is_counted_apart() {
        use std::os::unix::fs::PermissionsExt;
        let d = dirs();
        let u = package(&d.text, &texts(N));
        let off = run(&u, None);
        std::fs::set_permissions(&d.text, std::fs::Permissions::from_mode(0o770)).unwrap();
        let c = cache(&d, false);
        let got = run(&u, Some(&c));
        std::fs::set_permissions(&d.text, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(got, off);
        assert_eq!(count(&c, |c| &c.fill_text_unreadable), 1);
        assert_eq!(count(&c, |c| &c.fill_text_changed), 0);
        assert_eq!(count(&c, |c| &c.published_packages), 0);
    }

    /// A shared-tier hit (`skip_dependencies`) counts each skipped package as
    /// `inproc_reused_packages`, apart from disk hits.
    #[test]
    fn inproc_reuse_is_counted() {
        use crate::program::dep_summary::parse_for_build_observed;
        use crate::program::dep_summary::tests::ws_unit;
        use crate::program::profile::BuildProfile;
        let d = dirs();
        let ws = app_id("Ws");
        let snap = crate::snapshot::AppSetSnapshot {
            apps: vec![ws_unit(&ws), package(&d.text, &texts(N))],
            workspace_app: ws,
            world: crate::snapshot::World::Closed,
        };
        let c = cache(&d, false);
        parse_for_build_observed(&snap, BuildProfile::LIGHT, true, None, None, Some(&c));
        assert_eq!(count(&c, |c| &c.inproc_reused_packages), 1);
        assert_eq!(count(&c, |c| &c.seam_files), 0, "nothing reached the seam");
    }

    /// CDO's dependency units with their mapped text and digests, as an embedded-source
    /// snapshot of the pinned workspace loads them (ENG-113's text cache, the real one).
    fn cdo_dependency_units(ws: &Path) -> Vec<AppUnit> {
        let builder = crate::snapshot::SnapshotBuilder {
            workspace_root: ws.to_path_buf(),
            local_providers: vec![],
        };
        let (snap, _) = builder
            .build_with_options(
                crate::snapshot::DependencySource::Embedded,
                &crate::program::dep_cache::DepCache::default(),
            )
            .expect("CDO snapshot");
        let deps: Vec<AppUnit> = snap
            .apps
            .into_iter()
            .filter(|u| u.id != snap.workspace_app && u.source.is_some())
            .collect();
        assert!(
            deps.len() >= 10
                && deps
                    .iter()
                    .all(|u| u.source.as_ref().unwrap().text_digest.is_some()),
            "precondition: CDO's embedded-source dependencies, each mapped with a digest"
        );
        deps
    }

    /// Plan Task 11 (sol r5: any round-trip failure on CDO fails): every CDO dependency file
    /// goes through a real fill (owned text, round trip, envelope, publish) and a real hit
    /// (read, envelope, decode) on a temporary root; each pass gives exactly the fresh
    /// parse, every file is stored, nothing is refused.
    #[test]
    fn ir_cache_cdo_decoded_equals_parsed() {
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        let units = cdo_dependency_units(&ws);
        let tmp = tempfile::tempdir().unwrap();
        let ir = tmp.path().join("ir");
        let text = crate::snapshot::cache::cache_dir_path();
        let (mut files, mut cached_packages) = (0u64, 0u64);
        for u in &units {
            let n = u.source.as_ref().unwrap().files.len();
            files += n as u64;
            cached_packages += u64::from(n >= SMALL_PACKAGE_MIN_FILES);
            let off = run(u, None);
            let cold = IrCache::with_key(ir.clone(), text.clone(), false, KEY);
            assert!(run(u, Some(&cold)) == off, "{}: cold differs", u.id.name);
            let warm = IrCache::with_key(ir.clone(), text.clone(), false, KEY);
            assert!(run(u, Some(&warm)) == off, "{}: warm differs", u.id.name);
            for c in [&cold, &warm] {
                assert_eq!(count(c, |c| &c.roundtrip_failed), 0, "{}", u.id.name);
                assert_eq!(count(c, |c| &c.refused), 0, "{}", u.id.name);
                assert_eq!(count(c, |c| &c.fill_text_changed), 0, "{}", u.id.name);
                assert_eq!(count(c, |c| &c.fill_text_unreadable), 0, "{}", u.id.name);
                assert!(c.counters.balanced(), "{}", u.id.name);
            }
            if n >= SMALL_PACKAGE_MIN_FILES {
                assert_eq!(count(&cold, |c| &c.published_packages), 1, "{}", u.id.name);
                assert_eq!(count(&warm, |c| &c.disk_hits), n as u64, "{}", u.id.name);
                assert_eq!(count(&warm, |c| &c.skipped_unstored), 0, "{}", u.id.name);
            }
        }
        println!(
            "ir_cache CDO: {} packages ({cached_packages} cached), {files} files: cold and warm \
             equal the fresh parse, no round-trip failure",
            units.len()
        );
        assert!(
            files > 10_000,
            "precondition: CDO's ~10,900 dependency files"
        );
    }

    /// The CDO set encoded in two thread orders (forward and reversed `par_iter`) gives the
    /// same bytes for every file: an entry does not depend on which worker parsed what.
    #[test]
    fn ir_cache_cdo_encoding_is_deterministic() {
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        let mut files = 0usize;
        for u in cdo_dependency_units(&ws) {
            let src = &u.source.as_ref().unwrap().files;
            let enc = |order: Vec<usize>| -> Vec<(usize, Vec<u8>)> {
                let mut v: Vec<(usize, Vec<u8>)> =
                    crate::big_stack::big_stack_pool().install(|| {
                        order
                            .into_par_iter()
                            .map(|i| (i, encode_file(&al_syntax::parse(&src[i].text))))
                            .collect()
                    });
                v.sort_by_key(|(i, _)| *i);
                v
            };
            let forward = enc((0..src.len()).collect());
            let reverse = enc((0..src.len()).rev().collect());
            for ((i, a), (_, b)) in forward.iter().zip(&reverse) {
                assert!(
                    a == b,
                    "{} {}: bytes differ",
                    u.id.name,
                    src[*i].virtual_path
                );
            }
            files += src.len();
        }
        assert!(files > 10_000, "precondition: CDO's dependency files");
    }

    /// The switch: `on`/`off` in any case; anything else is off and warned (sol/Fable r1).
    #[test]
    fn the_switch_accepts_only_on_and_off() {
        use std::ffi::OsStr;
        let s = |v: Option<&str>| super::super::switch(v.map(OsStr::new));
        assert_eq!(s(None), (true, None));
        assert_eq!(s(Some("")), (true, None));
        assert_eq!(s(Some("ON")), (true, None));
        assert_eq!(s(Some("Off")), (false, None));
        for typo in ["of", "0", "false", "yes"] {
            let (on, warn) = s(Some(typo));
            assert!(!on && warn.is_some(), "{typo}");
        }
    }

    /// The child half of [`report_end_does_not_abort_an_unwind`].
    #[test]
    #[ignore = "run by report_end_does_not_abort_an_unwind as a child"]
    fn unwind_child_mode() {
        if std::env::var_os("ALSEM_IR_UNWIND_CHILD").is_none() {
            return;
        }
        let _report = super::super::install_from_env();
        let c = super::super::installed().expect("installed");
        c.first_use(None);
        // Counts that do not balance, as an interrupted run leaves them.
        Counters::add(&c.counters.seam_files, 1);
        panic!("unwinding with the report guard alive");
    }

    /// Fable r1: `report_end`'s balance assertion, in the guard's `Drop`, must not panic
    /// during an unwind (a second panic aborts the process): the run ends as a panic, and its
    /// end line is still written.
    #[test]
    fn report_end_does_not_abort_an_unwind() {
        let tmp = tempfile::tempdir().unwrap();
        let rep = tmp.path().join("report.jsonl");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "program::ir_cache::seam::tests::unwind_child_mode",
                "--test-threads=1",
            ])
            .env("ALSEM_IR_UNWIND_CHILD", "1")
            .env("ALSEM_IR_CACHE", "off")
            .env("ALSEM_IR_CACHE_REPORT", &rep)
            .env("ALSEM_IR_CACHE_DIR", tmp.path().join("ir"))
            .env("XDG_CACHE_HOME", tmp.path().join("xdg"))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(101),
            "a test panic, not an abort: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = std::fs::read_to_string(&rep).unwrap_or_default();
        assert!(text.contains("\"event\":\"end\""), "{text}");
    }
}
