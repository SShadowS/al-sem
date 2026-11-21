//! The IR cache's per-process counters (spec "Correctness checks").

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        /// One process's counts. Invariant: `seam_files = disk_hits + parsed_miss + refused +
        /// skipped_small + skipped_unstored + skipped_off`.
        #[derive(Default)]
        pub(crate) struct Counters {
            $(pub(crate) $name: AtomicU64,)*
        }

        impl Counters {
            /// The counts as a JSON object, in declaration order.
            pub(crate) fn snapshot(&self) -> serde_json::Value {
                let mut m = serde_json::Map::new();
                $(m.insert(stringify!($name).into(), self.$name.load(Relaxed).into());)*
                serde_json::Value::Object(m)
            }
        }
    };
}

counters!(
    seam_files,
    eligible_files,
    disk_hits,
    verified_hits,
    parsed_miss,
    refused,
    skipped_small,
    skipped_unstored,
    skipped_off,
    published_packages,
    roundtrip_failed,
    publish_skipped_busy,
    fill_text_changed,
    inproc_reused_packages,
);

impl Counters {
    pub(crate) fn add(c: &AtomicU64, n: usize) {
        c.fetch_add(n as u64, Relaxed);
    }

    /// Whether every seam file is accounted for once.
    pub(crate) fn balanced(&self) -> bool {
        let g = |c: &AtomicU64| c.load(Relaxed);
        g(&self.seam_files)
            == g(&self.disk_hits)
                + g(&self.parsed_miss)
                + g(&self.refused)
                + g(&self.skipped_small)
                + g(&self.skipped_unstored)
                + g(&self.skipped_off)
    }
}
