//! T-budget's unit half (design §4.5, §11): reservations against a meter
//! the test sets, each precondition stated by assignment.

use super::*;

fn budget(bytes: u64, rss: u64) -> (Budget, Arc<FakeMeter>) {
    let meter = Arc::new(FakeMeter::default());
    meter.rss.store(rss, Ordering::SeqCst);
    let b = Budget::new(bytes, meter.clone(), Estimates::new(false));
    (b, meter)
}

/// `rss + reserved + estimate <= budget`: a held reservation counts against
/// the next one until it drops; a fill's counts as a fill worker while held.
#[test]
fn a_reservation_counts_until_it_drops() {
    let (b, _) = budget(1_000, 100);
    let first = b.reserve(Case::Fill("fold"), 500).expect("fits: 100 + 500");
    assert_eq!(b.status().fill_workers, 1);
    let refused = b
        .reserve(Case::Trees, 500)
        .err()
        .expect("100 + 500 + 500 > 1000");
    assert_eq!((refused.reserved, refused.impossible), (500, false));
    drop(first);
    assert_eq!(b.status().fill_workers, 0);
    b.reserve(Case::Trees, 500)
        .expect("fits once the first is released");
}

/// An estimate that cannot fit above the retained floor is impossible: no
/// eviction could make room. The verdict follows the floor and the budget.
#[test]
fn an_estimate_above_the_floor_is_impossible_until_the_budget_grows() {
    let (b, _) = budget(1_000, 900);
    b.set_retained_floor(800);
    let f = b.reserve(Case::Fill("fold"), 300).err().unwrap();
    assert!(f.impossible, "800 + 300 > 1000");
    assert!(!b.possible(300));
    b.set_retained_floor(600);
    let f = b.reserve(Case::Fill("fold"), 300).err().unwrap();
    assert!(
        !f.impossible,
        "600 + 300 fits once 900 resident is reclaimed"
    );
    b.set_budget(2_000);
    b.reserve(Case::Fill("fold"), 300)
        .expect("a larger budget lifts it");
    assert_eq!(b.status().impossible, 1);
}

/// The pre-model estimate grows with the inputs, never below its floor; a
/// size-reducing edit lowers it.
#[test]
fn the_pre_model_estimate_follows_the_input_sizes() {
    let e = Estimates::new(false);
    let small = InputSizes {
        al_bytes: 1_000,
        package_bytes: 0,
    };
    assert_eq!(e.pre_model(small), ESTIMATE_FLOOR_CROSS_APP);
    let cdo = InputSizes {
        al_bytes: 4_970_000,
        package_bytes: 73_320_000,
    };
    let big = e.pre_model(cdo);
    assert_eq!(big, 4_970_000 * PER_AL_BYTE + 73_320_000 * PER_PACKAGE_BYTE);
    let edited = InputSizes {
        al_bytes: 4_000_000,
        ..cdo
    };
    assert!(e.pre_model(edited) < big);
    // Single-app reads no packages.
    assert_eq!(
        Estimates::new(true).pre_model(cdo),
        (4_970_000 * PER_AL_BYTE).max(ESTIMATE_FLOOR_SINGLE_APP)
    );
}

/// After the first build the table scales by routine count; an observed
/// peak replaces it, the largest of the last three.
#[test]
fn observed_peaks_replace_the_table() {
    let mut e = Estimates::new(false);
    e.routines = Some(TABLE_ROUTINES / 2);
    let none = InputSizes::default();
    assert_eq!(
        e.of(Case::Fill("fold"), none),
        900 * MIB * (TABLE_ROUTINES / 2) / TABLE_ROUTINES
    );
    for peak in [10, 40, 20, 30] {
        e.observe(Case::Fill("fold"), peak);
    }
    assert_eq!(
        e.of(Case::Fill("fold"), none),
        40,
        "40 is among the last three"
    );
    e.observe(Case::Fill("fold"), 5);
    assert_eq!(
        e.of(Case::Fill("fold"), none),
        30,
        "10 and 40 have aged out"
    );
}

/// A finished reservation records its peak over the RSS it started at, and
/// an RSS left above the budget counts an overrun.
#[test]
fn finishing_observes_the_peak_and_counts_an_overrun() {
    let (b, meter) = budget(1_000, 400);
    let r = b.reserve(Case::Trees, 100).unwrap();
    meter.peak.store(1_200, Ordering::SeqCst);
    meter.rss.store(1_100, Ordering::SeqCst);
    assert_eq!(r.finish(), 700, "left resident: 1100 - 400");
    assert_eq!(b.estimate(Case::Trees, InputSizes::default()), 800);
    assert_eq!(b.status().overruns, 1);
}

/// Estimates by size class: a large request's observed peak (a 699-root
/// impact's trees, 4174 MiB on CDO) is that class's estimate, never a small
/// request's (which then refused every later one-routine ask over a 5.7 GB
/// RSS); a small class keeps its own observations, or the table when it has
/// none; an unobserved larger class reads the larger of the table and the
/// smaller classes' peaks. A finished sized reservation observes in its
/// class.
///
/// DISCRIMINATION (2026-11-05): `of_sized` reading every class's peaks (the
/// class dropped from the lookup, as before) fails "one root still reads the
/// table" (4174 MiB against 900 MiB); restored, passes.
#[test]
fn a_large_requests_peak_is_not_a_small_requests_estimate() {
    let (b, meter) = budget(8 * GIB, 5 * GIB);
    let none = InputSizes::default();
    assert_eq!(size_class(0), 0);
    assert_eq!(size_class(1), 0);
    assert_eq!(size_class(4), 1);
    assert_eq!(size_class(5), 2);
    assert_eq!(size_class(699), 5);
    let table = b.estimate(Case::Trees, none);
    assert_eq!(table, 900 * MIB, "precondition: the table, no routines yet");
    // A 699-root request's trees peak at 4174 MiB over their start.
    let r = b.reserve_sized(Case::Trees, 699, table).unwrap();
    meter.peak.store(5 * GIB + 4174 * MIB, Ordering::SeqCst);
    r.finish();
    assert_eq!(b.estimate_sized(Case::Trees, none, 699), 4174 * MIB);
    assert_eq!(b.estimate_sized(Case::Trees, none, 400), 4174 * MIB);
    assert_eq!(
        b.estimate_sized(Case::Trees, none, 1),
        900 * MIB,
        "one root still reads the table"
    );
    assert_eq!(b.estimate(Case::Trees, none), 900 * MIB);
    // A one-root request observes in its own class.
    meter.peak.store(0, Ordering::SeqCst);
    let r = b.reserve_sized(Case::Trees, 1, table).unwrap();
    meter.peak.store(5 * GIB + 300 * MIB, Ordering::SeqCst);
    r.finish();
    assert_eq!(b.estimate_sized(Case::Trees, none, 1), 300 * MIB);
    assert_eq!(b.estimate_sized(Case::Trees, none, 699), 4174 * MIB);
    // An unobserved middle class: the table or a smaller class's peak.
    assert_eq!(b.estimate_sized(Case::Trees, none, 10), 900 * MIB);
    let mut e = Estimates::new(false);
    e.observe_sized(Case::Trees, 3, 1_000 * MIB);
    assert_eq!(e.of_sized(Case::Trees, none, 10), 1_000 * MIB);
    assert_eq!(e.of_sized(Case::Trees, none, 1), 900 * MIB);
}

/// The Windows meter's peak window (only a lifetime peak, never reset): a
/// window whose lifetime peak rose reports it exactly; one where it did not
/// reports 0, and a finished reservation then records no observation, so the
/// case keeps its table estimate instead of a made-up peak.
///
/// DISCRIMINATION (2026-11-02): `lifetime >= at_reset` in `window_peak`
/// reports an unrisen window's old peak: "did not rise: unknown" fails
/// (1500, not 0).
#[test]
fn an_unknown_window_peak_records_no_observation() {
    assert_eq!(window_peak(2_000, 1_500), 2_000, "rose: exact");
    assert_eq!(window_peak(1_500, 1_500), 0, "did not rise: unknown");
    assert_eq!(window_peak(1_000, 1_500), 0);
    let (b, meter) = budget(1_000_000_000_000, 400);
    let table = b.estimate(Case::Trees, InputSizes::default());
    let r = b.reserve(Case::Trees, 100).unwrap();
    meter
        .peak
        .store(window_peak(1_500, 1_500), Ordering::SeqCst);
    meter.rss.store(600, Ordering::SeqCst);
    assert_eq!(r.finish(), 200, "left resident: 600 - 400");
    assert_eq!(
        b.estimate(Case::Trees, InputSizes::default()),
        table,
        "an unknown peak changes no estimate"
    );
}

/// Closed admission holds a new holder until the closer reopens it; a
/// stopped request leaves; with no releaser in flight nothing waits.
#[test]
fn closed_admission_holds_new_holders_and_never_waits_without_a_releaser() {
    let (b, _) = budget(1_000, 0);
    let closed = b.close();
    assert!(b.admit(&|| true).is_none(), "a stopped request leaves");
    let start = std::time::Instant::now();
    b.wait_for_releasers(0, &|| false);
    assert!(start.elapsed() < std::time::Duration::from_millis(500));
    std::thread::scope(|s| {
        let waiter = s.spawn(|| b.admit(&|| false).is_some());
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(b.status().holders, 0, "held at the gate");
        drop(closed);
        assert!(waiter.join().unwrap());
    });
    assert_eq!(b.status().holders, 0, "its guard dropped");
}

#[test]
fn sizes_parse_with_binary_units() {
    assert_eq!(parse_bytes("4G").unwrap(), 4 * GIB);
    assert_eq!(parse_bytes("1.5GiB").unwrap(), 3 * GIB / 2);
    assert_eq!(parse_bytes("512m").unwrap(), 512 * MIB);
    assert_eq!(parse_bytes("1024").unwrap(), 1024);
    assert!(parse_bytes("lots").is_err());
    assert!(parse_bytes("0").is_err());
}

/// Engine limits B8: the `--memory-budget` help states the defaults
/// `default_budget` applies, each in a size `--memory-budget` itself takes.
#[test]
fn the_help_text_states_the_real_defaults() {
    let text = default_budget_text();
    let sizes: Vec<u64> = text
        .split(", ")
        .map(|part| parse_bytes(part.split(' ').next().unwrap()).unwrap())
        .collect();
    assert_eq!(
        sizes,
        [
            default_budget(false, false),
            default_budget(false, true),
            default_budget(true, false),
        ]
    );
    assert_eq!(
        text,
        "8G cross-app, 6G cross-app with --low-mem, 2G single-app"
    );
    assert_eq!(size_text(3 * GIB / 2), "1536M");
}
