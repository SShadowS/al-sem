# Provenance

The minimal repro of GitHub #245 (filed by CentralGauge against al-sem 2.0.1), copied
verbatim from the issue on 2026-10-23. `start/` has one interface; `final/` keeps it and
adds a second. Every interface used to get `<appId>:Interface:0`, so `alsem diff` of the
two fingerprints reported the new interface as a critical `procedure-signature-changed`
(#83). Read by `tests/cli/cli_numberless_object_ids.rs`
(`diff_issue_245_interface_added_fixture`).
