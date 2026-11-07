//! The output word table (output validation spec 2026-11-06, §8.1; V0,
//! approved by the owner as written on 2026-11-06). One spelling per idea in
//! renderer-owned human text, and in the JSON fields that carry a renamed
//! value beside its legacy field (§8.3: `tier: "onlyOpen"` stays,
//! `pathTier: "via-hub"` is added).
//!
//! Every renderer that prints a tier, an entry kind, an execution context, a
//! blocker kind, a hub or an absence verdict takes its words from here, so a
//! word changes in one place. [`REPLACED`] lists the words the table
//! replaces; `tests/cli/output_vocabulary.rs` checks that no fixture answer
//! prints one in renderer-owned text.

use crate::program::impact_graph::reverse::{EntryKind, Tier};

/// The tier of a path (§8.1): where it goes. Precedence is the weakest hop:
/// `via-hub` > `via-configuration` > `via-dependency` > `local`.
#[must_use]
pub fn tier(t: Tier) -> &'static str {
    match t {
        Tier::Local => "local",
        Tier::ThroughDependency => "via-dependency",
        Tier::OnlyOpen => "via-hub",
    }
}

/// The tier of a configured dispatch (workflow responses, report
/// selections, `roots.config.json` entries): its own tier, not a hub.
pub const VIA_CONFIGURATION: &str = "via-configuration";

/// A legacy tier key (`local`, `throughDependency`, `onlyOpen`, and the
/// tests' `configured`) in the §8.1 words; any other text unchanged.
#[must_use]
pub fn tier_key(key: &str) -> &str {
    match key {
        "throughDependency" | "through-dependency" | "dependency" => "via-dependency",
        "onlyOpen" | "only-open" | "dynamic" => "via-hub",
        "configured" => VIA_CONFIGURATION,
        k => k,
    }
}

/// The relation of a witness (§8.1): `code` when every edge is a call, run,
/// trigger or event; `UI navigation` when any edge is a page the user opens
/// from another page.
pub const RELATION_CODE: &str = "code";
pub const RELATION_NAVIGATION: &str = "UI navigation";

/// The entry kind of a legacy entry classification (§8.1, closed list): the
/// routine kind that starts a run. The legacy list mixed in contexts
/// (`api-page`, `web-service-exposed`, `job-queue-entrypoint`,
/// `background-start`); each of those is the kind of routine it starts
/// (`codeunit OnRun`, `page trigger`) plus a context ([`entry_context`]).
#[must_use]
pub fn entry_kind(k: EntryKind) -> &'static str {
    match k {
        EntryKind::PageAction => "page action",
        EntryKind::ApiPage | EntryKind::TriggerPage => "page trigger",
        EntryKind::ReportTrigger => "report trigger",
        EntryKind::TriggerTable => "table trigger",
        EntryKind::EventSubscriber => "event subscriber",
        EntryKind::InstallCodeunit => "install trigger",
        EntryKind::UpgradeCodeunit => "upgrade trigger",
        EntryKind::OnrunCodeunit
        | EntryKind::BackgroundStart
        | EntryKind::JobQueueEntrypoint
        | EntryKind::WebServiceExposed => "codeunit OnRun",
        EntryKind::TestProcedure => "test procedure",
    }
}

/// The plural of an entry kind, for a count line (`page actions 3`).
#[must_use]
pub fn entry_kind_plural(kind: &str) -> String {
    match kind {
        "codeunit OnRun" => "codeunit OnRun".to_string(),
        k => format!("{k}s"),
    }
}

/// The execution context an entry classification fixes (§8.1, closed
/// list), or `None` when the kind alone does not establish one.
#[must_use]
pub fn entry_context(k: EntryKind) -> Option<&'static str> {
    match k {
        EntryKind::BackgroundStart => Some("background session or task"),
        EntryKind::JobQueueEntrypoint => Some("background session or task (job queue)"),
        EntryKind::ApiPage => Some("API page"),
        EntryKind::WebServiceExposed => Some("web service"),
        EntryKind::InstallCodeunit => Some("install"),
        EntryKind::UpgradeCodeunit => Some("upgrade"),
        EntryKind::TestProcedure => Some("test run"),
        _ => None,
    }
}

/// A legacy entry-kind key (`page-action`, `trigger-page`, …) in the §8.1
/// words: `(kind, context)`. Unknown keys pass through as the kind.
#[must_use]
pub fn entry_key(key: &str) -> (&str, Option<&'static str>) {
    match EntryKind::ALL.iter().find(|k| k.as_str() == key) {
        Some(k) => (entry_kind(*k), entry_context(*k)),
        None => (key, None),
    }
}

/// A blocker kind's words (§8.1, closed list): the callers' context kinds
/// and the existing-effects kinds. JSON keeps its keys; any key not in the
/// list is printed with its dashes as spaces.
#[must_use]
pub fn blocker(key: &str) -> String {
    let w = match key {
        "external-callers" => "external callers",
        "hidden-callers" => "hidden callers",
        "only-open" => "over a hub",
        "order-unknown" => "order not resolved",
        "publication-unknown" => "publication data not loaded",
        "unread-publication-file" => "publication file not read",
        "unrecognized-guard" => "guard not recognized",
        "unresolved-background-target" => "background target not resolved",
        "effects-incomplete" => "effects incomplete",
        "tryfunction-caller" => "TryFunction caller",
        "limit" => "limit",
        "unresolved-callsite" => "call not resolved",
        "open-world-dispatch" => "interface dispatch (open world)",
        "non-unconditional-effect-exists" => "conditional effect exists",
        "analysis-gap" => "analysis gap",
        "cone-truncated" => "cone truncated",
        "witness-truncated" => "witness truncated",
        "coverage-incomplete" => "coverage incomplete",
        "temp-state-depends-on-caller" => "caller-dependent write",
        "unknown-table-write" => "write on a table not resolved",
        "slice-search-capped" => "site search capped",
        "evidence-unavailable" => "evidence unavailable",
        other => return other.replace('-', " "),
    };
    w.to_string()
}

/// The hub kind of a graph hub id (`h:run:codeunit`, `h:record:modify`, …),
/// §8.1's closed list plus the classes the export has that the list did not
/// name (`h:any`, `h:name:…`, `h:callbacks:…`). `None` for a non-hub id.
#[must_use]
pub fn hub_kind(id: &str) -> Option<String> {
    let rest = id.strip_prefix("h:")?;
    let (class, arg) = rest.split_once(':').unwrap_or((rest, ""));
    Some(match class {
        "run" => match arg {
            "codeunit" => "Codeunit.Run of a variable".to_string(),
            "page" => "Page.Run of a variable".to_string(),
            "report" => "Report.Run of a variable".to_string(),
            "xmlport" => "Xmlport.Run of a variable".to_string(),
            "query" => "Query.Run of a variable".to_string(),
            "" => "Run of an object variable".to_string(),
            k => format!("{k} Run of a variable"),
        },
        "record" => format!("record trigger ({})", capitalize(arg)),
        // Round-3 c02: a RunTrigger-false operation on a table not resolved:
        // the platform's table events only, never a trigger.
        "record-events" => format!("table events ({}, no trigger)", capitalize(arg)),
        "any" => "call not resolved (any routine)".to_string(),
        "name" => {
            let name = arg.split('/').next().unwrap_or(arg);
            format!("call by name {name} (open world)")
        }
        "callbacks" => "callbacks into the app (opaque body)".to_string(),
        other => format!("{other} dispatch"),
    })
}

/// A hub in the §8.1 form: `[hub <kind> <name>? at <file>:<line>]`; the
/// place is left out when there is none.
#[must_use]
pub fn hub(kind: &str, name: Option<&str>, at: Option<&str>) -> String {
    let mut s = format!("[hub {kind}");
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        s.push(' ');
        s.push_str(n);
    }
    if let Some(a) = at {
        s.push_str(" at ");
        s.push_str(a);
    }
    s.push(']');
    s
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The negative-certainty words (§8.1) of an absence verdict: proven absent
/// under a complete analysis.
pub const NO_EVERY_CANDIDATE_DECIDED: &str = "no (every candidate decided)";

/// The verb a record write prints (round-2 output validation G39: an
/// operation prints its own verb): `delete` for `Delete`/`DeleteAll`,
/// `insert`, `modify` for `Modify`/`ModifyAll`, `rename`; `change` when the
/// kinds mix (or none is known).
#[must_use]
pub fn write_verb<'k>(kinds: impl IntoIterator<Item = &'k str>) -> &'static str {
    let verb_of = |k: &str| match k {
        "Delete" | "DeleteAll" => "delete",
        "Insert" => "insert",
        "Modify" | "ModifyAll" => "modify",
        "Rename" => "rename",
        _ => "change",
    };
    let mut verb: Option<&'static str> = None;
    for k in kinds {
        let v = verb_of(k);
        match verb {
            None => verb = Some(v),
            Some(w) if w == v => {}
            Some(_) => return "change",
        }
    }
    verb.unwrap_or("change")
}

/// `none observed (searched: …; not searched: …)`: nothing found by a partial
/// analysis or an index search, with its search.
#[must_use]
pub fn none_observed(searched: &str, not_searched: &str) -> String {
    if not_searched.is_empty() {
        format!("none observed (searched: {searched})")
    } else {
        format!("none observed (searched: {searched}; not searched: {not_searched})")
    }
}

/// An absence verdict: [`NO_EVERY_CANDIDATE_DECIDED`] when the analysis is
/// complete, else [`none_observed`] with its search.
#[must_use]
pub fn absence(complete: bool, searched: &str, not_searched: &str) -> String {
    if complete {
        NO_EVERY_CANDIDATE_DECIDED.to_string()
    } else {
        none_observed(searched, not_searched)
    }
}

/// The completeness words of an analysis (§8.1), always with a subject:
/// `analysis complete` or `analysis partial (<reasons>)`.
#[must_use]
pub fn analysis(complete: bool, reasons: &[String]) -> String {
    match (complete, reasons.is_empty()) {
        (true, _) => "analysis complete".to_string(),
        (false, true) => "analysis partial".to_string(),
        (false, false) => format!("analysis partial ({})", reasons.join(", ")),
    }
}

/// The condition of an effect (§8.1) for a legacy conditionality key
/// (`unconditional-on-success`, `loop-body`, `conditional`, `unknown`).
#[must_use]
pub fn condition(key: &str) -> &str {
    match key {
        "unconditional-on-success" | "unconditional" => "unconditional on success",
        "loop-body" | "in-loop" => "in a loop",
        "conditional" => "conditional",
        "unknown" => "condition not resolved",
        k => k,
    }
}

/// The test status of a routine (§8.1) for a legacy status key.
#[must_use]
pub fn test_status(key: &str) -> &str {
    match key {
        "reached" => "tests with a modeled path",
        "no-known-path" => "no modeled path",
        "analysis-incomplete" => "analysis partial",
        "not-in-projection" => "not in projection",
        "not-loaded" => "not loaded",
        "not-requested" => "not requested",
        k => k,
    }
}

/// The words §8.1 replaces, as they would appear in renderer-owned human
/// text. Each entry is matched as a substring; the vocabulary test reads
/// renderer-owned lines only (it skips quoted source and JSON).
pub const REPLACED: &[&str] = &[
    // tier
    "onlyOpen",
    "only-open",
    "only over open sites",
    "through an open site (a hub)",
    "throughDependency",
    "through-dependency",
    "tier configured",
    "localApproximate",
    // hub kinds
    "h:run:",
    "h:record:",
    "h:record-events:",
    "h:any",
    "h:name:",
    "h:callbacks:",
    "HUB:",
    // entry kinds and contexts
    "page-action",
    "trigger-page",
    "page-event",
    "trigger-table",
    "report-trigger",
    "onrun-codeunit",
    "install-codeunit",
    "upgrade-codeunit",
    "background-start",
    // blocker kinds
    "external-callers",
    "order-unknown",
    "publication-unknown",
    "unread-publication-file",
    "unrecognized-guard",
    "unresolved-background-target",
    "effects-incomplete",
    "tryfunction-caller",
    "unresolved-callsite",
    "open-world-dispatch",
    "non-unconditional-effect-exists",
    // dependency boundary
    "entry point",
    // negative certainty
    "writer(s)",
    "no observed",
    "no indexed consumer observed",
    // target resolution
    "isUnknownBucket",
    "decided at run time",
    // temp state
    "known(true)",
    "known(false)",
    "parameter-dependent(",
    "temp state unknown",
    // condition
    "(unknown)",
    // completeness and test status
    "coverage: complete",
    "no-known-path",
    "analysis-incomplete",
    "not-in-projection",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// No replaced word is a word of the table itself (a check on the lists).
    #[test]
    fn the_new_words_are_not_replaced_words() {
        let mut new: Vec<String> = vec![
            tier(Tier::Local).into(),
            tier(Tier::ThroughDependency).into(),
            tier(Tier::OnlyOpen).into(),
            VIA_CONFIGURATION.into(),
        ];
        for k in EntryKind::ALL {
            new.push(entry_kind(k).into());
            new.extend(entry_context(k).map(str::to_string));
        }
        for k in [
            "external-callers",
            "only-open",
            "order-unknown",
            "unresolved-callsite",
            "open-world-dispatch",
            "non-unconditional-effect-exists",
        ] {
            new.push(blocker(k));
        }
        for id in [
            "h:run:codeunit",
            "h:record:modify",
            "h:record-events:modify",
            "h:any",
            "h:name:x/1",
        ] {
            new.push(hub_kind(id).unwrap());
        }
        for w in &new {
            for r in REPLACED {
                assert!(!w.contains(r), "{w} contains the replaced word {r}");
            }
        }
    }
}
