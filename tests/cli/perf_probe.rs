//! `scripts/perf-probe` (TLS-73): it must refuse when samply's ring buffers cannot
//! all be mapped, and name the locked-memory fix. On 2026-11-15 it passed (one
//! event, a 16-page buffer) while a real `samply record` failed with "mmap failed".

fn probe(pages: &str) -> (Option<i32>, String) {
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/perf-probe"))
        .env("PERF_PROBE_PAGES", pages)
        // One CPU: one buffer, whatever the machine's CPU count.
        .env("PERF_PROBE_CPUS", "1")
        .output()
        .expect("spawn bash scripts/perf-probe");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code(), text)
}

/// One buffer larger than this user's whole perf mlock budget
/// (kernel.perf_event_mlock_kb x online CPUs, plus `ulimit -l`) cannot map, so
/// the probe must say BLOCKED and name the host fix. Where perf events cannot
/// be opened at all (no CAP_PERFMON, paranoid > 1, no `cc`), the probe refuses
/// earlier and only its exit code is checked. Where the kernel skips the mlock
/// check (CAP_IPC_LOCK, paranoid -1, an unlimited `ulimit -l`) the test does
/// not run the probe, which would really allocate the buffer there.
///
/// Mutation (2026-11-15): the C probe ignoring its page count (always 16
/// pages, as before TLS-73) -> every buffer maps, exit 0, the test fails.
#[cfg(target_os = "linux")]
#[test]
fn perf_probe_refuses_when_samply_sized_buffers_do_not_map() {
    // Exempt or unlimited: the buffer would really be allocated, so do not ask.
    if mlock_check_exempt() {
        return;
    }
    let Some(budget_kb) = mlock_budget_kb() else {
        return;
    };
    // The smallest power-of-two page count (4 KiB pages) past the budget. The
    // kernel refuses it before allocating anything.
    let pages = (budget_kb / 4 + 1).next_power_of_two();
    if pages > 9_999_999 {
        return; // past the probe's own cap
    }
    let (code, text) = probe(&pages.to_string());
    if text.contains("cannot build the probe") {
        return;
    }
    assert_eq!(code, Some(1), "{pages} pages: {text}");
    if text.contains("perf_event_open failed") && !text.contains("ring buffer failed") {
        return;
    }
    assert!(text.contains("ring buffer failed"), "{text}");
    assert!(text.contains("perf_event_mlock_kb=4096"), "{text}");
}

/// This user's perf mlock budget in KiB, or `None` when `ulimit -l` is unlimited.
#[cfg(target_os = "linux")]
fn mlock_budget_kb() -> Option<u64> {
    let per_cpu: u64 = std::fs::read_to_string("/proc/sys/kernel/perf_event_mlock_kb")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let sh = |c: &str| {
        let out = std::process::Command::new("bash")
            .args(["-c", c])
            .output()
            .ok()?;
        String::from_utf8(out.stdout).ok()
    };
    let cpus: u64 = sh("getconf _NPROCESSORS_ONLN")?.trim().parse().ok()?;
    let memlock: u64 = sh("ulimit -l")?.trim().parse().ok()?;
    Some(per_cpu * cpus + memlock)
}

/// CAP_IPC_LOCK (bit 14 of CapEff) or perf_event_paranoid -1: the kernel does
/// not limit perf buffers' locked memory (uid 0 alone does not exempt).
#[cfg(target_os = "linux")]
fn mlock_check_exempt() -> bool {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .map(|v| v.split_whitespace().next().unwrap_or("").to_string())
    };
    let ipc_lock = field("CapEff:")
        .and_then(|h| u64::from_str_radix(&h, 16).ok())
        .is_some_and(|c| c & (1 << 14) != 0);
    let paranoid = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .is_ok_and(|p| p.trim() == "-1");
    ipc_lock || paranoid
}

#[cfg(target_os = "linux")]
#[test]
fn perf_probe_rejects_a_bad_page_count() {
    let (code, text) = probe("0");
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("PERF_PROBE_PAGES"), "{text}");
}
