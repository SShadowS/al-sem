//! `format_html` — port of al-sem `src/cli/format-html.ts`.
//!
//! Produces the self-contained HTML report for `--format html`.
//! Byte-parity with the TS reference: identical HTML structure, escaping,
//! whitespace, and SVG coordinates.
//!
//! ## SVG coordinate parity
//! All SVG coordinates are integers, produced by INTENTIONAL integer arithmetic
//! that matches TS's emitted values exactly. Two integer-division sites:
//!   - bezier midpoints `mx = (x1 + x2) / 2` — every (x1,x2) pair used has an even
//!     sum (266+400, 650+760, …), so the halve is exact.
//!   - `y_of`: `top = (h_total - block_h) / 2 + 20` — `h_total - block_h` is always
//!     even (both are multiples of ROW=46 plus/minus the even constant 40), so the
//!     halve is exact.
//!
//! Parity holds while those divisors stay even. DO NOT "fix" either to floating
//! point — that would re-introduce the float-formatting divergence we avoid.
//! The fixed-literal attributes `stroke-width="1.5"` and `opacity="0.7"` are
//! NOT arithmetic — they are hardcoded string constants.
//!
//! ## Event graph parity
//! The `renderEventGraph` port uses `IndexMap` for `byLoc`, `subsByEvent`, `pubY`,
//! `subY` so insertion order matches the TS `new Map(...)` iteration order.
//! The budget branch (MAX_EVENTS/MAX_EDGES/MAX_SUBSCRIBERS) is implemented but
//! is unexercised by the corpus.
//!
//! ## HTML escaping
//! Matches the TS `h()` function exactly:
//!   `&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;`, `"` → `&quot;`, `'` → `&#39;`

use indexmap::IndexMap;

use crate::engine::gate::app_attribution::App;
use crate::engine::gate::projection::FindingSummary;
use crate::engine::l3::coverage::AnalysisCoverage;
use crate::engine::l3::event_graph::{EventGraph, EventSymbol};
use crate::engine::l3::l3_workspace::{L3Object, L3Resolved, L3Routine, L3Table};
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l5::finding::Finding;

// ---------------------------------------------------------------------------
// Severity / confidence colour palettes (mirrors format-html.ts)
// ---------------------------------------------------------------------------

const SEV_ORDER: &[&str] = &["critical", "high", "medium", "low", "info"];

fn sev_color(sev: &str) -> &'static str {
    match sev {
        "critical" => "oklch(0.52 0.20 25)",
        "high" => "oklch(0.62 0.18 45)",
        "medium" => "oklch(0.74 0.14 80)",
        "low" => "oklch(0.62 0.11 240)",
        _ => "oklch(0.62 0.02 255)", // info + fallback
    }
}

fn sev_fg(sev: &str) -> &'static str {
    match sev {
        "medium" => "oklch(0.26 0.04 80)",
        _ => "oklch(0.99 0 0)",
    }
}

fn conf_color(level: &str) -> &'static str {
    match level {
        "confirmed" => "oklch(0.58 0.13 150)",
        "likely" => "oklch(0.60 0.11 240)",
        _ => "oklch(0.72 0.13 80)", // possible + fallback
    }
}

const SAFETY_RANK_HIGH: i32 = 3;
const SAFETY_RANK_MEDIUM: i32 = 2;
const SAFETY_RANK_LOW: i32 = 1;

fn safety_rank(s: &str) -> i32 {
    match s {
        "high" => SAFETY_RANK_HIGH,
        "medium" => SAFETY_RANK_MEDIUM,
        "low" => SAFETY_RANK_LOW,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// HTML escape — mirrors `h()` in format-html.ts exactly
// ---------------------------------------------------------------------------

fn h(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// String helpers — mirrors TS helpers
// ---------------------------------------------------------------------------

/// `shortFile(sourceUnitId)` — return the part after the first `:`, or the
/// whole string if no `:`.
fn short_file(source_unit_id: &str) -> &str {
    match source_unit_id.find(':') {
        Some(i) => &source_unit_id[i + 1..],
        None => source_unit_id,
    }
}

/// `trunc(s, n)` — truncate to n chars (replacing the last char with `…`).
///
/// UTF-16 FOLLOW-UP: this slices on Unicode scalar boundaries (`chars()`), but TS
/// `trunc` (format-html.ts:56-58) measures/slices on UTF-16 code units
/// (`s.length` / `s.slice`). For non-BMP labels (astral chars in eventName n=30,
/// pub n=30, sub n=32, routineLabel n=24) the cut point diverges. This is part of
/// the tracked engine-wide UTF-16 `compareStrings` follow-up — DO NOT change the
/// behavior in isolation. When the swap lands it must use UTF-16 code-unit
/// semantics for slicing (cut on an `encode_utf16()` boundary, re-decoding the
/// kept prefix), NOT byte- or scalar-based slicing, to actually match al-sem.
/// Corpus-invisible today (all labels are BMP).
fn trunc(s: &str, n: usize) -> String {
    // Engine never panics: guard n==0 (unreachable today — callers pass 24/30/32 —
    // but `chars[..n - 1]` would underflow otherwise).
    if n == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > n {
        let truncated: String = chars[..n - 1].iter().collect();
        format!("{truncated}\u{2026}")
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Model lookup maps (mirrors `buildMaps` + helper functions)
// ---------------------------------------------------------------------------

struct Maps<'a> {
    routines: std::collections::HashMap<&'a str, &'a L3Routine>,
    objects: std::collections::HashMap<&'a str, &'a L3Object>,
    tables: std::collections::HashMap<&'a str, &'a L3Table>,
}

impl<'a> Maps<'a> {
    fn build(routines: &'a [L3Routine], objects: &'a [L3Object], tables: &'a [L3Table]) -> Self {
        Maps {
            routines: routines.iter().map(|r| (r.id.as_str(), r)).collect(),
            objects: objects.iter().map(|o| (o.id.as_str(), o)).collect(),
            // G-5: REAL table wins an id collision with a tableextension stub.
            tables: crate::engine::l3::l3_workspace::table_by_id_preferring_real(tables),
        }
    }
}

/// `routineLabel(routineId, m)` — `"ObjectName :: RoutineName"` or truncated id.
fn routine_label(routine_id: &str, m: &Maps) -> String {
    let r = m.routines.get(routine_id);
    match r {
        None => trunc(routine_id, 24),
        Some(r) => {
            let o = m.objects.get(r.object_id.as_str());
            match o {
                Some(o) => format!("{} :: {}", o.name, r.name),
                None => r.name.clone(),
            }
        }
    }
}

/// `tableLabel(tableId, m)` — table name or last `/`-segment.
fn table_label(table_id: &str, m: &Maps) -> String {
    if let Some(t) = m.tables.get(table_id) {
        return t.name.clone();
    }
    let parts: Vec<&str> = table_id.split('/').collect();
    parts.last().copied().unwrap_or(table_id).to_string()
}

/// `anchorLine(a)` — `"shortFile:line"` (1-based line).
fn anchor_line(source_unit_id: &str, start_line: u32) -> String {
    format!("{}:{}", short_file(source_unit_id), start_line + 1)
}

// ---------------------------------------------------------------------------
// renderFlow — mirrors `renderFlow` in format-html.ts
// ---------------------------------------------------------------------------

fn render_flow(finding: &Finding, m: &Maps) -> String {
    // Through `evidence_path_of`: a cohort-bearing d1 finding derives its path
    // from `cohort_contexts[0].witness` rather than storing it.
    let steps = crate::engine::l5::finding::evidence_path_of(finding);
    let steps = &*steps;
    if steps.is_empty() {
        return String::new();
    }
    let len = steps.len();
    let mut nodes = String::new();
    for (i, step) in steps.iter().enumerate() {
        let last = i == len - 1;
        // Badge — exactly like TS: loopId → badge-loop, callsiteId → badge-call,
        // else-if-last → badge-op, else "".
        let badge = if step.loop_id.is_some() {
            r#"<span class="badge badge-loop">↻ loop</span>"#.to_string()
        } else if step.callsite_id.is_some() {
            r#"<span class="badge badge-call">calls</span>"#.to_string()
        } else if last {
            r#"<span class="badge badge-op">db op</span>"#.to_string()
        } else {
            String::new()
        };
        let label = routine_label(&step.routine_id, m);
        let loc = anchor_line(
            &step.source_anchor.source_unit_id,
            step.source_anchor.start_line,
        );
        let class = if last {
            "flow-step is-terminal"
        } else {
            "flow-step"
        };
        nodes.push_str(&format!(
            "\n      <li class=\"{class}\">\n        <span class=\"flow-rail\"><span class=\"flow-dot\"></span></span>\n        <span class=\"flow-body\">\n          <span class=\"flow-head\">{} {badge}</span>\n          <span class=\"flow-note\">{}</span>\n          <span class=\"flow-loc\">{}</span>\n        </span>\n      </li>",
            h(&label),
            h(&step.note),
            h(&loc),
        ));
    }
    // For a d1 COHORT finding, render the compressed per-verdict-class summary
    // (each class's loop_count) instead of the old "+ N other paths" note — the
    // representative witness above is ONE class's; this names how many loops each
    // class covers. Non-d1 findings keep the additional-paths note unchanged.
    let extra = if let Some(cohorts) = &finding.cohort_contexts {
        let total_loops: u64 = cohorts.iter().map(|c| c.loop_count).sum();
        let n = cohorts.len();
        let class_noun = if n == 1 { "class" } else { "classes" };
        let loop_noun = if total_loops == 1 { "loop" } else { "loops" };
        let items: String = cohorts
            .iter()
            .map(|c| {
                let ln = if c.loop_count == 1 { "loop" } else { "loops" };
                let unc = if c.uncertain { ", uncertain" } else { "" };
                format!(
                    "\n        <li>{} \u{2014} {} {ln} (severity {}{})</li>",
                    h(&c.verdict),
                    c.loop_count,
                    h(&c.severity),
                    unc
                )
            })
            .collect();
        format!(
            r#"<p class="flow-extra">Reached by {total_loops} {loop_noun} across {n} verdict {class_noun}:</p><ul class="cohort-classes">{items}</ul>"#
        )
    } else {
        match &finding.additional_paths {
            Some(paths) if !paths.is_empty() => {
                let n = paths.len();
                let noun = if n == 1 { "path" } else { "paths" };
                format!(r#"<p class="flow-extra">+ {n} other {noun} reach the same operation</p>"#)
            }
            _ => String::new(),
        }
    };
    format!("<ol class=\"flow\">{nodes}</ol>{extra}")
}

// ---------------------------------------------------------------------------
// renderFinding — mirrors `renderFinding` in format-html.ts
//
// The TS template literal is:
// ```
// return `
//   <article class="finding" data-sev="${sev}" style="--sev:${SEV_COLOR[sev]}">
//     <header class="finding-head">
//       <span class="sev-dot"></span>
//       <code class="detector">${h(finding.detector)}</code>
//       <h3>${h(finding.title)}</h3>
//       <span class="conf" style="--conf:${confColor}">${h(conf.level)}${h(capped)}</span>
//     </header>
//     <p class="root-cause">${h(finding.rootCause)}</p>
//     ${renderFlow(finding, m)}
//     ${tables ? `<div class="tables">...</div>` : ""}
//     ${fixes ? `<details class="fix">...</details>` : ""}
//     ${co}
//   </article>`;
// ```
//
// Key: in a TS template literal `\n    ${expr}` the `\n    ` is ALWAYS emitted,
// even when `expr` is `""`. So a missing tables/fixes/co produces `\n    ` (just
// the indent), not nothing. Two consecutive empty template slots produce `\n    \n    `.
// ---------------------------------------------------------------------------

fn render_finding(
    finding: &Finding,
    _summary: &FindingSummary,
    m: &Maps,
    co_located: &[String],
) -> String {
    let sev = &finding.severity;
    let conf_level = &finding.confidence.level;
    let conf_color_val = conf_color(conf_level);

    // cappedBy suffix
    let capped = match &finding.confidence.capped_by {
        Some(cb) if !cb.is_empty() => format!(" · capped by {}", cb.join(", ")),
        _ => String::new(),
    };

    // affectedTables — use raw finding tables (projected via tableLabel)
    let tables_html: String = finding
        .affected_tables
        .iter()
        .map(|t| format!("<span class=\"chip\">{}</span>", h(&table_label(t, m))))
        .collect();

    // fixOptions — sorted by safety desc (STABLE)
    let mut fix_options = finding.fix_options.clone();
    fix_options.sort_by_key(|x| std::cmp::Reverse(safety_rank(&x.safety)));
    let fixes_html: String = fix_options
        .iter()
        .map(|f| {
            format!(
                "<li><span class=\"safety safety-{}\">{}</span> {}</li>",
                h(&f.safety),
                h(&f.safety),
                h(&f.description),
            )
        })
        .collect();

    // co-located — TS: `${co}` where co is `<div...>` or `""`.
    let co_html = if !co_located.is_empty() {
        let codes: String = co_located
            .iter()
            .map(|d| format!("<code>{}</code>", h(d)))
            .collect::<Vec<_>>()
            .join(" ");
        format!("<div class=\"co\">co-located: {codes}</div>")
    } else {
        String::new()
    };

    let flow_html = render_flow(finding, m);

    // tables: `<div class="tables">...</div>` or `""` (TS: tables ? ... : "")
    let tables_section = if !tables_html.is_empty() {
        format!("<div class=\"tables\"><span class=\"lbl\">writes</span>{tables_html}</div>")
    } else {
        String::new()
    };

    // fixes: `<details...>` or `""` (TS: fixes ? ... : "")
    let fixes_section = if !fixes_html.is_empty() {
        format!(
            "<details class=\"fix\"><summary>Fix options</summary><ul>{fixes_html}</ul></details>"
        )
    } else {
        String::new()
    };

    // Mirror the exact TS template literal — each `\n    ${expr}` emits `\n    ` + expr,
    // even when expr is "". This produces the golden's exact whitespace.
    format!(
        "\n  <article class=\"finding\" data-sev=\"{sev}\" style=\"--sev:{sev_color}\">\n    <header class=\"finding-head\">\n      <span class=\"sev-dot\"></span>\n      <code class=\"detector\">{detector}</code>\n      <h3>{title}</h3>\n      <span class=\"conf\" style=\"--conf:{conf_color_val}\">{conf_level_h}{capped_h}</span>\n    </header>\n    <p class=\"root-cause\">{root_cause_h}</p>\n    {flow_html}\n    {tables_section}\n    {fixes_section}\n    {co_html}\n  </article>",
        sev_color = sev_color(sev),
        detector = h(&finding.detector),
        title = h(&finding.title),
        conf_level_h = h(conf_level),
        capped_h = h(&capped),
        root_cause_h = h(&finding.root_cause),
    )
}

// ---------------------------------------------------------------------------
// renderEventGraph — mirrors `renderEventGraph` in format-html.ts
// ---------------------------------------------------------------------------

const MAX_EVENTS: usize = 40;
const MAX_EDGES: usize = 500;
const MAX_SUBSCRIBERS: usize = 200;

/// `bezier(x1, y1, x2, y2, color)` — mirrors the TS `bezier` function.
fn bezier(x1: i64, y1: i64, x2: i64, y2: i64, color: &str) -> String {
    let mx = (x1 + x2) / 2;
    format!(
        "<path d=\"M {x1} {y1} C {mx} {y1}, {mx} {y2}, {x2} {y2}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\" opacity=\"0.7\"/>"
    )
}

/// `node(x, y, w, hgt, label, fill, stroke, tag?, dead?, full?)` — mirrors TS `node`.
#[allow(clippy::too_many_arguments)] // SVG node geometry+style params; grouping would obscure
fn node_svg(
    x: i64,
    y: i64,
    w: i64,
    hgt: i64,
    label: &str,
    fill: &str,
    stroke: &str,
    tag: Option<&str>,
    dead: bool,
    full: Option<&str>,
) -> String {
    let title_text = h(full.unwrap_or(label));
    let tag_svg = match tag {
        Some(t) => {
            let fill_attr = if dead {
                sev_color("high")
            } else {
                "oklch(0.55 0.02 255)"
            };
            format!(
                "<text x=\"{}\" y=\"{}\" class=\"g-tag\" text-anchor=\"end\" fill=\"{fill_attr}\">{}</text>",
                x + w - 8,
                y + hgt / 2 + 4,
                h(t),
            )
        }
        None => String::new(),
    };
    format!(
        "<g class=\"g-node\"><title>{title_text}</title><rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{hgt}\" rx=\"7\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\"/><text x=\"{}\" y=\"{}\" class=\"g-label\">{}</text>{tag_svg}</g>",
        x + 12,
        y + hgt / 2 + 4,
        h(label),
    )
}

/// Sort comparator for event/publisher/subscriber labels (mirrors TS `cmp`,
/// format-html.ts:38: `a < b ? -1 : a > b ? 1 : 0`).
///
/// UTF-16 FOLLOW-UP: `str::cmp` orders by UTF-8 bytes (== Unicode scalar order),
/// but TS `<`/`>` on strings orders by UTF-16 code units. These agree for BMP but
/// diverge for non-BMP (surrogate-pair) chars. This is part of the tracked
/// engine-wide UTF-16 `compareStrings` follow-up — DO NOT change in isolation.
/// When it lands it must compare via `encode_utf16().cmp(...)`, NOT bytes/scalars,
/// to match al-sem for non-BMP labels. Corpus-invisible today (all labels are BMP).
fn cmp_str(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}

/// `pubLabel(ev, m)` — owning object name or publisherObjectId.
fn pub_label(ev: &EventSymbol, m: &Maps) -> String {
    match m.objects.get(ev.publisher_object_id.as_str()) {
        Some(o) => o.name.clone(),
        None => ev.publisher_object_id.clone(),
    }
}

fn render_event_graph(graph: &EventGraph, m: &Maps) -> String {
    if graph.events.is_empty() {
        return String::new();
    }

    // Sort events: (eventName, publisherObjectId, id) — mirrors TS comparator chain.
    let mut events: Vec<&EventSymbol> = graph.events.iter().collect();
    events.sort_by(|a, b| {
        cmp_str(&a.event_name, &b.event_name)
            .then_with(|| cmp_str(&a.publisher_object_id, &b.publisher_object_id))
            .then_with(|| cmp_str(&a.id, &b.id))
    });

    let event_id_set: std::collections::HashSet<&str> =
        events.iter().map(|e| e.id.as_str()).collect();
    let graph_edges: Vec<&crate::engine::l3::event_graph::EventEdge> = graph
        .edges
        .iter()
        .filter(|e| event_id_set.contains(e.event_id.as_str()))
        .collect();
    let subscriber_count = graph_edges
        .iter()
        .map(|e| e.subscriber_routine_id.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Budget check
    if events.len() > MAX_EVENTS
        || graph_edges.len() > MAX_EDGES
        || subscriber_count > MAX_SUBSCRIBERS
    {
        return format!(
            "\n  <section class=\"graph-wrap\">\n    <h2>Event graph</h2>\n    <p class=\"sub\">Graph omitted: {} events · {} links · {} subscribers exceed the inline render limit ({MAX_EVENTS}/{MAX_EDGES}/{MAX_SUBSCRIBERS}). Use <code>events fanout</code> / <code>events chains</code> for the full data.</p>\n  </section>",
            events.len(),
            graph_edges.len(),
            subscriber_count,
        );
    }

    // subsByEvent: insertion-ordered map (event_id → Vec<subscriber_routine_id>)
    let mut subs_by_event: IndexMap<String, Vec<String>> = IndexMap::new();
    for e in &graph_edges {
        let arr = subs_by_event.entry(e.event_id.clone()).or_default();
        arr.push(e.subscriber_routine_id.clone());
    }
    // Sort subscriber ids per event: by routineLabel then by id (deterministic)
    for subs in subs_by_event.values_mut() {
        subs.sort_by(|a, b| {
            let la = routine_label(a, m);
            let lb = routine_label(b, m);
            cmp_str(&la, &lb).then_with(|| cmp_str(a, b))
        });
    }

    // Publisher column: distinct pub labels, sorted.
    // (Dedup order is immaterial post-sort; mirrors the subs_col pattern below.)
    let pubs: Vec<String> = {
        let mut v: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for ev in &events {
            let p = pub_label(ev, m);
            if seen.insert(p.clone()) {
                v.push(p);
            }
        }
        v.sort_by(|a, b| cmp_str(a, b));
        v
    };

    // Subscriber column: distinct subscriber labels, sorted
    let subs_col: Vec<String> = {
        let mut sub_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for subs in subs_by_event.values() {
            for s in subs {
                sub_set.insert(routine_label(s, m));
            }
        }
        let mut v: Vec<String> = sub_set.into_iter().collect();
        v.sort_by(|a, b| cmp_str(a, b));
        v
    };

    const ROW: i64 = 46;
    const NODE_H: i64 = 30;
    const W: i64 = 1040;
    const COL_X_PUB: i64 = 16;
    const COL_X_EV: i64 = 400;
    const COL_X_SUB: i64 = 760;
    const COL_W_PUB: i64 = 250;
    const COL_W_EV: i64 = 250;
    const COL_W_SUB: i64 = 264;

    let rows = pubs.len().max(events.len()).max(subs_col.len()).max(1) as i64;
    let h_total = rows * ROW + 40;

    // yOf(i, count) — mirrors TS `yOf`. The `/ 2` is INTENTIONAL integer division:
    // `h_total - block_h` is always even (both are multiples of ROW=46 ± the even
    // constant 40), so it halves exactly and matches TS. See the module-level
    // "SVG coordinate parity" note — DO NOT convert to floating point.
    let y_of = |i: i64, count: i64| -> i64 {
        let block_h = count * ROW;
        let top = (h_total - block_h) / 2 + 20;
        top + i * ROW
    };

    // pubY and subY: IndexMap for insertion-order (mirrors TS `new Map(...)`)
    let pub_y: IndexMap<String, i64> = pubs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.clone(), y_of(i as i64, pubs.len() as i64)))
        .collect();
    let sub_y: IndexMap<String, i64> = subs_col
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), y_of(i as i64, subs_col.len() as i64)))
        .collect();

    let mut edges_svg: Vec<String> = Vec::new();
    let mut nodes_svg: Vec<String> = Vec::new();

    for (i, ev) in events.iter().enumerate() {
        let ey = y_of(i as i64, events.len() as i64);
        let subs_for_ev = subs_by_event
            .get(ev.id.as_str())
            .cloned()
            .unwrap_or_default();
        let dead = subs_for_ev.is_empty();
        let ev_center_y = ey + NODE_H / 2;

        // publisher → event edge
        let pl = pub_label(ev, m);
        let py = pub_y.get(&pl).copied().unwrap_or(ey) + NODE_H / 2;
        edges_svg.push(bezier(
            COL_X_PUB + COL_W_PUB,
            py,
            COL_X_EV,
            ev_center_y,
            "oklch(0.78 0.02 255)",
        ));

        // event → subscriber edges
        for s in &subs_for_ev {
            let sl = routine_label(s, m);
            let sy = sub_y.get(&sl).copied().unwrap_or(ey) + NODE_H / 2;
            edges_svg.push(bezier(
                COL_X_EV + COL_W_EV,
                ev_center_y,
                COL_X_SUB,
                sy,
                "oklch(0.7 0.08 240)",
            ));
        }

        let fill = if dead {
            "oklch(0.96 0.04 25)"
        } else {
            "oklch(0.97 0.02 240)"
        };
        let stroke = if dead {
            sev_color("high")
        } else {
            "oklch(0.62 0.10 240)"
        };
        let tag = if dead {
            format!("{} subs", subs_for_ev.len())
        } else {
            format!("{}", subs_for_ev.len())
        };
        nodes_svg.push(node_svg(
            COL_X_EV,
            ey,
            COL_W_EV,
            NODE_H,
            &trunc(&ev.event_name, 30),
            fill,
            stroke,
            Some(&tag),
            dead,
            Some(&ev.event_name),
        ));
    }

    for p in &pubs {
        let y = pub_y.get(p).copied().unwrap_or(0);
        nodes_svg.push(node_svg(
            COL_X_PUB,
            y,
            COL_W_PUB,
            NODE_H,
            &trunc(p, 30),
            "oklch(0.98 0.005 255)",
            "oklch(0.78 0.02 255)",
            None,
            false,
            Some(p),
        ));
    }
    for s in &subs_col {
        let y = sub_y.get(s).copied().unwrap_or(0);
        nodes_svg.push(node_svg(
            COL_X_SUB,
            y,
            COL_W_SUB,
            NODE_H,
            &trunc(s, 32),
            "oklch(0.98 0.01 240)",
            "oklch(0.7 0.08 240)",
            None,
            false,
            Some(s),
        ));
    }

    let headers = format!(
        "\n    <text x=\"{COL_X_PUB}\" y=\"16\" class=\"g-col\">PUBLISHER</text>\n    <text x=\"{COL_X_EV}\" y=\"16\" class=\"g-col\">EVENT</text>\n    <text x=\"{COL_X_SUB}\" y=\"16\" class=\"g-col\">SUBSCRIBER</text>"
    );

    let edges_joined = edges_svg.join("\n      ");
    let nodes_joined = nodes_svg.join("\n      ");

    format!(
        "\n  <section class=\"graph-wrap\">\n    <h2>Event graph</h2>\n    <p class=\"sub\">Publishers fan out to subscribers across files. Events outlined in red have no subscribers (dead extension points).</p>\n    <svg viewBox=\"0 0 {W} {h_total}\" class=\"evgraph\" role=\"img\" aria-label=\"Event publisher to subscriber graph\">\n      {headers}\n      {edges_joined}\n      {nodes_joined}\n    </svg>\n  </section>"
    )
}

// ---------------------------------------------------------------------------
// CSS style block (verbatim from STYLE constant in format-html.ts)
// ---------------------------------------------------------------------------

const STYLE: &str = r#"
:root{
  --bg:oklch(0.99 0.004 255);--surface:oklch(1 0 0);--border:oklch(0.91 0.008 255);
  --ink:oklch(0.30 0.02 260);--muted:oklch(0.52 0.015 260);--accent:oklch(0.55 0.14 262);
  --mono:"SFMono-Regular",ui-monospace,"JetBrains Mono",Menlo,Consolas,monospace;
  --sans:ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font-family:var(--sans);line-height:1.5;
  -webkit-font-smoothing:antialiased}
.wrap{max-width:1080px;margin:0 auto;padding:48px 28px 96px}
.masthead{display:flex;flex-wrap:wrap;align-items:baseline;gap:8px 16px;
  border-bottom:1px solid var(--border);padding-bottom:20px;margin-bottom:8px}
.masthead h1{font-size:1.7rem;font-weight:680;letter-spacing:-0.02em;margin:0}
.masthead h1 b{color:var(--accent)}
.masthead .app{font-family:var(--mono);font-size:0.85rem;color:var(--muted)}
.coverage{color:var(--muted);font-size:0.84rem;margin:14px 0 26px}
.tally{display:flex;height:34px;border-radius:8px;overflow:hidden;border:1px solid var(--border);margin-bottom:6px}
.tally span{display:flex;align-items:center;justify-content:center;color:oklch(0.99 0 0);
  font-size:0.78rem;font-weight:600;min-width:34px;font-variant-numeric:tabular-nums}
.tally-legend{display:flex;flex-wrap:wrap;gap:14px;font-size:0.78rem;color:var(--muted);margin-bottom:40px}
.tally-legend i{display:inline-block;width:9px;height:9px;border-radius:3px;margin-right:5px;vertical-align:baseline}
.sev-group{margin:0 0 14px}
.sev-group>h2{font-size:0.78rem;text-transform:uppercase;letter-spacing:0.1em;color:var(--muted);
  font-weight:700;margin:34px 0 12px}
.finding{background:var(--surface);border:1px solid var(--border);border-radius:12px;
  padding:18px 20px;margin:0 0 14px}
.finding-head{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.sev-dot{width:11px;height:11px;border-radius:50%;background:var(--sev);flex:none;
  box-shadow:0 0 0 4px color-mix(in oklch,var(--sev) 16%,transparent)}
.detector{font-family:var(--mono);font-size:0.76rem;color:var(--muted);
  background:oklch(0.96 0.006 260);padding:2px 7px;border-radius:5px}
.finding-head h3{font-size:1.02rem;font-weight:620;margin:0;flex:1 1 auto;letter-spacing:-0.01em}
.conf{font-size:0.72rem;font-weight:600;color:var(--conf);
  border:1px solid color-mix(in oklch,var(--conf) 40%,transparent);
  background:color-mix(in oklch,var(--conf) 10%,transparent);padding:2px 9px;border-radius:20px;white-space:nowrap}
.root-cause{color:var(--ink);margin:11px 0 4px;max-width:74ch}
.flow{list-style:none;margin:16px 0 4px;padding:0}
.flow-step{display:flex;gap:14px;position:relative}
.flow-rail{flex:none;width:14px;display:flex;justify-content:center;position:relative}
.flow-rail::before{content:"";position:absolute;top:0;bottom:0;width:2px;background:var(--border)}
.flow-step:first-child .flow-rail::before{top:11px}
.flow-step:last-child .flow-rail::before{bottom:calc(100% - 11px)}
.flow-dot{width:11px;height:11px;border-radius:50%;background:var(--surface);
  border:2px solid var(--muted);margin-top:5px;z-index:1}
.flow-step.is-terminal .flow-dot{background:var(--sev);border-color:var(--sev);
  box-shadow:0 0 0 4px color-mix(in oklch,var(--sev) 16%,transparent)}
.flow-body{display:flex;flex-direction:column;padding-bottom:18px;gap:1px}
.flow-head{font-weight:580;font-size:0.92rem}
.flow-note{color:var(--muted);font-size:0.86rem}
.flow-loc{font-family:var(--mono);font-size:0.76rem;color:var(--accent)}
.flow-extra{color:var(--muted);font-size:0.8rem;margin:2px 0 0 28px}
.badge{font-size:0.66rem;font-weight:700;text-transform:uppercase;letter-spacing:0.04em;
  padding:1px 6px;border-radius:5px;vertical-align:middle;margin-left:4px}
.badge-loop{background:oklch(0.93 0.06 80);color:oklch(0.45 0.12 70)}
.badge-call{background:oklch(0.95 0.01 260);color:var(--muted)}
.badge-op{background:color-mix(in oklch,var(--sev) 18%,transparent);color:var(--sev)}
.tables{display:flex;align-items:center;flex-wrap:wrap;gap:6px;margin:12px 0 2px}
.tables .lbl{font-size:0.72rem;text-transform:uppercase;letter-spacing:0.06em;color:var(--muted);margin-right:2px}
.chip{font-family:var(--mono);font-size:0.76rem;background:oklch(0.96 0.01 260);
  border:1px solid var(--border);padding:2px 8px;border-radius:6px}
.fix{margin-top:12px}
.fix summary{font-size:0.84rem;font-weight:600;cursor:pointer;color:var(--accent)}
.fix ul{margin:8px 0 0;padding-left:2px;list-style:none}
.fix li{margin:6px 0;font-size:0.88rem;color:var(--ink)}
.safety{font-size:0.66rem;font-weight:700;text-transform:uppercase;padding:1px 6px;border-radius:5px;margin-right:6px}
.safety-high{background:oklch(0.92 0.07 150);color:oklch(0.42 0.12 155)}
.safety-medium{background:oklch(0.93 0.06 80);color:oklch(0.45 0.12 70)}
.safety-low{background:oklch(0.93 0.05 30);color:oklch(0.48 0.14 30)}
.co{margin-top:11px;font-size:0.78rem;color:var(--muted)}
.co code{font-family:var(--mono);background:oklch(0.96 0.006 260);padding:1px 5px;border-radius:4px}
.graph-wrap{margin-top:56px;border-top:1px solid var(--border);padding-top:8px}
.graph-wrap h2{font-size:1.15rem;font-weight:640;letter-spacing:-0.01em;margin:24px 0 4px}
.graph-wrap .sub{color:var(--muted);font-size:0.86rem;margin:0 0 18px;max-width:70ch}
.evgraph{width:100%;height:auto;background:var(--surface);border:1px solid var(--border);border-radius:12px;padding:8px}
.g-col{font-family:var(--sans);font-size:11px;font-weight:700;letter-spacing:0.1em;fill:oklch(0.6 0.02 260)}
.g-label{font-family:var(--sans);font-size:12.5px;font-weight:540;fill:var(--ink)}
.g-tag{font-family:var(--mono);font-size:11px;font-weight:700}
.empty{color:var(--muted);font-style:italic;margin:40px 0}
.wrap footer{margin-top:56px;color:var(--muted);font-size:0.78rem;border-top:1px solid var(--border);padding-top:16px}
"#;

// ---------------------------------------------------------------------------
// formatHtml — the public entry point
// ---------------------------------------------------------------------------

/// Inputs needed by the HTML formatter (assembled from the gate pipeline).
pub struct HtmlFormatInputs<'a> {
    /// Post-filter, post-scope, post-limit findings (pre-sorted).
    pub findings: &'a [(FindingSummary, &'a Finding)],
    /// The resolved workspace model.
    pub resolved: &'a L3Resolved,
    /// Coverage statistics.
    pub coverage: &'a AnalysisCoverage,
    /// Primary app identity (from workspace `app.json`).
    pub primary_app: Option<&'a App>,
}

/// Build the HTML report string (WITHOUT trailing `\n` — the caller appends one,
/// matching al-sem's `process.stdout.write(\`${formatHtml(...)}\n\`)`).
pub fn format_html(inputs: &HtmlFormatInputs<'_>) -> String {
    let ws = &inputs.resolved.workspace;
    let m = Maps::build(&ws.routines, &ws.objects, &ws.tables);

    let findings = inputs.findings;
    let cov = inputs.coverage;
    let app = inputs.primary_app;

    // Count by severity
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for sev in SEV_ORDER {
        counts.insert(sev, 0);
    }
    for (summary, _raw) in findings {
        *counts.entry(summary.severity.as_str()).or_insert(0) += 1;
    }
    // co-location map: "sourceUnitId:startLine:startColumn" → Vec<detector>
    // Uses IndexMap so insertion order is preserved (mirrors TS `new Map()`).
    // NOTE: keys off the RAW `Finding.primary_location` (NOT the projected
    // `FindingSummary.primary_location`). When a finding has an `actionable_anchor`,
    // `project_finding` swaps primary←actionable and demotes the original to the
    // terminal location, so the projected primary ≠ the raw primary. TS `locKey`
    // (format-html.ts:436-437) keys on `f.primaryLocation.{sourceUnitId,range.startLine,
    // range.startColumn}` — the raw anchor — so we must too, or co-location grouping
    // diverges for any finding that carries an actionable anchor.
    let loc_key = |raw: &Finding| -> String {
        format!(
            "{}:{}:{}",
            raw.primary_location.source_unit_id,
            raw.primary_location.start_line,
            raw.primary_location.start_column
        )
    };
    let mut by_loc: IndexMap<String, Vec<String>> = IndexMap::new();
    for (summary, raw) in findings {
        let k = loc_key(raw);
        by_loc.entry(k).or_default().push(summary.detector.clone());
    }

    // tally bar — only non-zero severities
    let tally: String = SEV_ORDER
        .iter()
        .filter(|&&s| counts.get(s).copied().unwrap_or(0) > 0)
        .map(|&s| {
            let c = counts.get(s).copied().unwrap_or(0);
            format!(
                "<span style=\"flex:{c};background:{bg};color:{fg}\" title=\"{s}\">{c}</span>",
                bg = sev_color(s),
                fg = sev_fg(s),
            )
        })
        .collect();

    // legend — all 5 severities (even if count is 0)
    let legend: String = SEV_ORDER
        .iter()
        .map(|&s| {
            let c = counts.get(s).copied().unwrap_or(0);
            format!(
                "<span><i style=\"background:{}\"></i>{s} {c}</span>",
                sev_color(s),
            )
        })
        .collect();

    // groups per severity
    let groups: String = SEV_ORDER
        .iter()
        .map(|&sev| {
            let fs: Vec<(&FindingSummary, &Finding)> = findings
                .iter()
                .filter(|(s, _)| s.severity == sev)
                .map(|(s, r)| (s, *r))
                .collect();
            if fs.is_empty() {
                return String::new();
            }
            let cards: String = fs
                .iter()
                .map(|(summary, raw)| {
                    // Look up co-located detectors by the RAW finding's key (see loc_key note).
                    let k = loc_key(raw);
                    // TS: `[...new Set(co)]` — deduplicate while preserving insertion order.
                    let raw_co = by_loc
                        .get(&k)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|d| d.as_str() != summary.detector.as_str())
                        .collect::<Vec<_>>();
                    // Insertion-order dedup (mirrors JS `new Set(arr)` → first occurrence wins).
                    let co: Vec<String> = {
                        let mut seen = std::collections::HashSet::new();
                        raw_co
                            .into_iter()
                            .filter(|d| seen.insert(d.clone()))
                            .collect()
                    };
                    render_finding(raw, summary, &m, &co)
                })
                .collect();
            let n = fs.len();
            format!("<section class=\"sev-group\"><h2>{sev} ({n})</h2>{cards}</section>")
        })
        .collect();

    // body
    let body = if findings.is_empty() {
        r#"<p class="empty">No findings. (Absence of a finding is not absence of a problem — see coverage.)</p>"#
            .to_string()
    } else {
        groups
    };

    // app masthead line
    let app_line = match app {
        Some(a) => format!(
            "<span class=\"app\">{} · {} · {}</span>",
            h(&a.name),
            h(&a.version),
            h(&a.publisher),
        ),
        None => String::new(),
    };

    // title
    let title = match app {
        Some(a) => format!("al-sem report — {}", h(&a.name)),
        None => "al-sem report".to_string(),
    };

    // Event graph
    // Engine-switch S6.8: the model's own event graph (the program engine's on the
    // analyze path).
    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let graph = crate::engine::l3::event_graph::events_for(inputs.resolved, &symbols);
    let event_graph_html = render_event_graph(&graph, &m);

    let finding_count = findings.len();

    // Coverage line
    let cov_line = format!(
        "{} routines ({} with bodies, {} parse-incomplete) ·\n    {}/{} source units parsed ·\n    {} opaque app(s)",
        cov.routines_total,
        cov.routines_body_available,
        cov.routines_parse_incomplete.len(),
        cov.source_units_parsed,
        cov.source_units_total,
        cov.opaque_apps.len(),
    );

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<div class=\"wrap\">\n  <div class=\"masthead\">\n    <h1><b>al-sem</b> analysis report</h1>\n    {app_line}\n  </div>\n  <div class=\"coverage\">\n    {cov_line}\n  </div>\n  <div class=\"tally\">{tally}</div>\n  <div class=\"tally-legend\">{legend}</div>\n  {body}\n  {event_graph_html}\n  <footer>Generated by al-sem · static semantic analysis for AL · {finding_count} finding(s)</footer>\n</div>\n</body>\n</html>"
    )
}

// ---------------------------------------------------------------------------
// Unit tests — native oracles for the corpus-invisible over-budget branch
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l3::event_graph::{EventEdge, EventGraph, EventSymbol};
    use crate::engine::l3::l3_workspace::{L3Object, L3Routine};
    use crate::engine::l5::finding::{FindingConfidence, SourceAnchor};

    fn empty_maps() -> Maps<'static> {
        // Using leaked empty slices for 'static lifetime in tests
        Maps {
            routines: std::collections::HashMap::new(),
            objects: std::collections::HashMap::new(),
            tables: std::collections::HashMap::new(),
        }
    }

    fn make_event(id: &str, publisher_object_id: &str, event_name: &str) -> EventSymbol {
        EventSymbol {
            id: id.to_string(),
            publisher_object_id: publisher_object_id.to_string(),
            publisher_routine_id: None,
            publisher_stable_routine_id: None,
            event_name: event_name.to_string(),
            event_kind: "integration".to_string(),
            element_name: None,
            signature_hash: "abc".to_string(),
            parameters: vec![],
            isolated: None,
            provenance: vec![],
            publisher_ref: None,
        }
    }

    fn make_edge(event_id: &str, subscriber_routine_id: &str) -> EventEdge {
        EventEdge {
            event_id: event_id.to_string(),
            subscriber_routine_id: subscriber_routine_id.to_string(),
            subscriber_stable_routine_id: subscriber_routine_id.to_string(),
            subscriber_app_id: "app1".to_string(),
            resolution: "resolved".to_string(),
            provenance: vec![],
        }
    }

    // ---------------------------------------------------------------------------
    // Oracle 1: h() — HTML escape function
    // ---------------------------------------------------------------------------
    #[test]
    fn html_escape_all_special_chars() {
        assert_eq!(h("&"), "&amp;");
        assert_eq!(h("<"), "&lt;");
        assert_eq!(h(">"), "&gt;");
        assert_eq!(h("\""), "&quot;");
        assert_eq!(h("'"), "&#39;");
        assert_eq!(h("hello world"), "hello world");
        assert_eq!(
            h("<script>&'\"</script>"),
            "&lt;script&gt;&amp;&#39;&quot;&lt;/script&gt;"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 2: over-budget branch — > MAX_EVENTS triggers the fallback template
    // ---------------------------------------------------------------------------
    #[test]
    fn over_budget_events_emits_fallback_template() {
        let m = empty_maps();
        // Create MAX_EVENTS + 1 events (41 events > MAX_EVENTS=40)
        let events: Vec<EventSymbol> = (0..=MAX_EVENTS as u32)
            .map(|i| make_event(&format!("ev{i}"), &format!("obj{i}"), &format!("Event{i}")))
            .collect();
        let graph = EventGraph {
            events,
            edges: vec![],
        };
        let html = render_event_graph(&graph, &m);
        assert!(
            html.contains("Graph omitted:"),
            "over-budget events must emit the fallback template: {html}"
        );
        assert!(
            html.contains(&format!("{MAX_EVENTS}/{MAX_EDGES}/{MAX_SUBSCRIBERS}")),
            "fallback must include budget limits: {html}"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 3: over-budget branch — > MAX_EDGES triggers the fallback template
    // ---------------------------------------------------------------------------
    #[test]
    fn over_budget_edges_emits_fallback_template() {
        let m = empty_maps();
        // Create 1 event and MAX_EDGES + 1 edges
        let ev = make_event("ev0", "obj0", "EventFoo");
        let events = vec![ev];
        let edges: Vec<EventEdge> = (0..=MAX_EDGES as u32)
            .map(|i| make_edge("ev0", &format!("sub{i}")))
            .collect();
        let graph = EventGraph { events, edges };
        let html = render_event_graph(&graph, &m);
        assert!(
            html.contains("Graph omitted:"),
            "over-budget edges must emit the fallback template: {html}"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 4: over-budget branch — > MAX_SUBSCRIBERS triggers fallback
    // ---------------------------------------------------------------------------
    #[test]
    fn over_budget_subscribers_emits_fallback_template() {
        let m = empty_maps();
        // 2 events each with 101 distinct subscribers = 202 > MAX_SUBSCRIBERS=200
        let ev0 = make_event("ev0", "obj0", "EventA");
        let ev1 = make_event("ev1", "obj1", "EventB");
        let events = vec![ev0, ev1];
        let edges: Vec<EventEdge> = (0..=MAX_SUBSCRIBERS as u32)
            .map(|i| make_edge("ev0", &format!("sub{i}")))
            .collect();
        // MAX_SUBSCRIBERS + 1 distinct subscribers > MAX_SUBSCRIBERS
        let graph = EventGraph { events, edges };
        let html = render_event_graph(&graph, &m);
        assert!(
            html.contains("Graph omitted:"),
            "over-budget subscribers must emit the fallback template: {html}"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 5: empty event graph → no <section> block
    // ---------------------------------------------------------------------------
    #[test]
    fn empty_event_graph_returns_empty() {
        let m = empty_maps();
        let graph = EventGraph {
            events: vec![],
            edges: vec![],
        };
        let html = render_event_graph(&graph, &m);
        assert!(
            html.is_empty(),
            "empty event graph must return empty string: {html}"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 6: bezier — integer coordinate formatting
    // ---------------------------------------------------------------------------
    #[test]
    fn bezier_integer_coordinates() {
        let b = bezier(266, 55, 400, 55, "oklch(0.78 0.02 255)");
        assert_eq!(
            b,
            "<path d=\"M 266 55 C 333 55, 333 55, 400 55\" fill=\"none\" stroke=\"oklch(0.78 0.02 255)\" stroke-width=\"1.5\" opacity=\"0.7\"/>",
            "bezier must produce the exact golden coordinate string"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 7: trunc — mirrors TS trunc behavior
    // ---------------------------------------------------------------------------
    #[test]
    fn trunc_at_boundary() {
        // 5 chars → no truncation
        assert_eq!(trunc("hello", 5), "hello");
        // 6 chars with n=5 → 4 chars + ellipsis
        assert_eq!(trunc("hello!", 5), "hell\u{2026}");
        // exactly n → no truncation
        assert_eq!(trunc("abcde", 5), "abcde");
        // MINOR-4 guard: n==0 never panics (unreachable today, but must be safe).
        assert_eq!(trunc("abcde", 0), "");
        assert_eq!(trunc("", 0), "");
    }

    // ---------------------------------------------------------------------------
    // Oracle 8: co-location keys off the RAW primary location, not the projected
    // one (MUST-FIX 1). Two findings share the SAME raw primary anchor, but the
    // FIRST carries an `actionable_anchor` pointing at a DIFFERENT location — so its
    // PROJECTED `FindingSummary.primary_location` differs from the raw one. TS
    // `locKey` keys on the raw anchor, so the two must CO-LOCATE. Keying on the
    // projected summary (the pre-fix bug) would split them apart.
    // This test FAILS if `loc_key` is reverted to key off `FindingSummary`.
    // ---------------------------------------------------------------------------
    fn anchor(source_unit_id: &str, line: u32, col: u32, routine_id: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: source_unit_id.to_string(),
            start_line: line,
            start_column: col,
            end_line: line,
            end_column: col + 1,
            enclosing_routine_id: routine_id.to_string(),
            syntax_kind: "call_statement".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn minimal_finding(
        id: &str,
        detector: &str,
        primary: SourceAnchor,
        actionable: Option<SourceAnchor>,
    ) -> Finding {
        Finding {
            id: id.to_string(),
            root_cause_key: id.to_string(),
            detector: detector.to_string(),
            title: format!("title {id}").into(),
            root_cause: format!("root cause {id}"),
            severity: "medium".to_string(),
            confidence: FindingConfidence {
                level: "likely".to_string(),
                capped_by: None,
                evidence: vec![],
            },
            primary_location: primary,
            evidence_path: vec![],
            additional_paths: None,
            affected_objects: vec![],
            affected_tables: vec![],
            fix_options: vec![],
            provenance: vec![],
            actionable_anchor: actionable,
            fingerprint: Some(id.to_string()),
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
        }
    }

    #[test]
    fn co_location_keys_off_raw_not_projected_primary() {
        use crate::engine::gate::projection::{ProjectionIndex, project_finding};
        use crate::engine::l3::coverage::AnalysisCoverage;
        use crate::engine::l3::l3_workspace::{L3Resolved, L3Workspace};

        // Both findings sit at the SAME raw primary anchor (ws:Foo.al, line 9, col 4).
        let raw_primary_a = anchor("ws:Foo.al", 9, 4, "rA");
        let raw_primary_b = anchor("ws:Foo.al", 9, 4, "rB");
        // Finding A carries an actionable anchor at a DIFFERENT location (line 42),
        // so its PROJECTED primary becomes line 43 — diverging from the raw line 10.
        let actionable_a = anchor("ws:Bar.al", 42, 0, "rA");

        let f_a = minimal_finding("fA", "d1-db-op-in-loop", raw_primary_a, Some(actionable_a));
        let f_b = minimal_finding("fB", "d4-repeated-lookup-in-loop", raw_primary_b, None);

        // Project (empty model — display names resolve to None, irrelevant to the key).
        let objects: Vec<L3Object> = vec![];
        let routines: Vec<L3Routine> = vec![];
        let idx = ProjectionIndex::build(&objects, &routines);
        let sum_a = project_finding(&f_a, &idx);
        let sum_b = project_finding(&f_b, &idx);

        // Sanity: A's projected primary really did move off the raw anchor.
        assert_ne!(
            sum_a.primary_location.line, sum_b.primary_location.line,
            "precondition: actionable_anchor must make A's projected primary differ from B's"
        );

        let resolved = L3Resolved {
            workspace: L3Workspace {
                objects: vec![],
                tables: vec![],
                routines: vec![],
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![],
            precomputed_calls: None,
            precomputed_events: None,
        };
        let coverage = AnalysisCoverage {
            source_units_total: 1,
            source_units_parsed: 1,
            routines_total: 0,
            routines_body_available: 0,
            routines_parse_incomplete: vec![],
            opaque_apps: vec![],
            unresolved_callsites: vec![],
            dynamic_dispatch_sites: vec![],
        };

        let findings: Vec<(FindingSummary, &Finding)> = vec![(sum_a, &f_a), (sum_b, &f_b)];
        let html = format_html(&HtmlFormatInputs {
            findings: &findings,
            resolved: &resolved,
            coverage: &coverage,
            primary_app: None,
        });

        // Because the two share the SAME raw primary, each finding card must list the
        // OTHER detector as co-located. (Keying off the projected summary would split
        // them — A's projected line ≠ B's projected line — and emit NO co-located block.)
        assert!(
            html.contains(
                r#"<div class="co">co-located: <code>d4-repeated-lookup-in-loop</code></div>"#
            ),
            "finding A must show d4 as co-located (raw-key match):\n{html}"
        );
        assert!(
            html.contains(r#"<div class="co">co-located: <code>d1-db-op-in-loop</code></div>"#),
            "finding B must show d1 as co-located (raw-key match):\n{html}"
        );
    }

    // ---------------------------------------------------------------------------
    // Oracle 9: missing primary_app (None) — corpus-invisible (all 22 disk fixtures
    // have an app.json → Some). TS: `appLine = ""` and the title carries no app
    // suffix (`<title>al-sem report</title>`). Mirrors format-html.ts:472-474,481.
    // ---------------------------------------------------------------------------
    #[test]
    fn missing_primary_app_renders_empty_masthead_and_bare_title() {
        use crate::engine::l3::coverage::AnalysisCoverage;
        use crate::engine::l3::l3_workspace::{L3Resolved, L3Workspace};

        let resolved = L3Resolved {
            workspace: L3Workspace {
                objects: vec![],
                tables: vec![],
                routines: vec![],
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![],
            precomputed_calls: None,
            precomputed_events: None,
        };
        let coverage = AnalysisCoverage {
            source_units_total: 0,
            source_units_parsed: 0,
            routines_total: 0,
            routines_body_available: 0,
            routines_parse_incomplete: vec![],
            opaque_apps: vec![],
            unresolved_callsites: vec![],
            dynamic_dispatch_sites: vec![],
        };
        let findings: Vec<(FindingSummary, &Finding)> = vec![];
        let html = format_html(&HtmlFormatInputs {
            findings: &findings,
            resolved: &resolved,
            coverage: &coverage,
            primary_app: None,
        });

        // Bare title (no ` — <app>` suffix).
        assert!(
            html.contains("<title>al-sem report</title>"),
            "missing primary_app must render the bare title:\n{html}"
        );
        // appLine == "" → masthead has the h1 then the empty template slot, NO
        // `<span class="app">`.
        assert!(
            !html.contains(r#"<span class="app">"#),
            "missing primary_app must NOT render an app span:\n{html}"
        );
        // The h1 is still present.
        assert!(
            html.contains("<h1><b>al-sem</b> analysis report</h1>"),
            "masthead h1 must always render:\n{html}"
        );
    }
}
