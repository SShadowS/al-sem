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
