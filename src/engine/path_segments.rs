//! Path segments (output validation spec 2026-11-06, §7.5, F45): a run of
//! hops that several printed paths share prints once, under a stable segment
//! id. The first path that uses a segment prints `[segment S1: 7 hops]`, a
//! later one `[S1]`, and the hops are listed once, under `path segments`,
//! after the last line of the region. A path keeps its own last hop (the
//! effect, the routine asked about), and every hop keeps what it carries: a
//! hub, a guard or a location prints on the hop that has it, inside the
//! segment's listing.
//!
//! Every human answer that prints several paths uses it over its whole
//! answer: explain's card, prove's witnesses and evidence rows, digest's
//! per-root card and query effects. It replaces the per-section shared start
//! of MCP output step 2 (`(in <section>, a path that begins with `… →`
//! begins with: …)`), which named one start per section and printed it again
//! in the next section.
//!
//! [`expand`] is the inverse: it rebuilds every path line in full from the
//! listing. The semantic gate of §12.1 (shared evidence reconstructs each
//! witness's hops through its segment) checks it against the renderers.

use std::collections::HashMap;

/// The separator between two hops of a printed path.
pub const SEP: &str = " → ";

/// The shortest run of hops worth a segment: a two-hop run saves less than
/// the reference costs to read.
const MIN_HOPS: usize = 3;

/// The header line of the listing.
pub const LISTING: &str = "path segments (each [Sn] in a path above stands for these hops):";

/// A path line: its indentation, its label (`path: `, `prefix path: `,
/// `representative path (…): `, a tagged `path […]: `) and its hops.
fn path_line(l: &str) -> Option<(usize, &str, Vec<&str>)> {
    let indent = l.len() - l.trim_start().len();
    let t = l.trim_start();
    let end = t.find(": ")? + 2;
    let label = &t[..end];
    let is_path = label == "path: "
        || label == "prefix path: "
        || (label.starts_with("representative path (") && !label.contains(SEP))
        || (label.starts_with("path [") && label.ends_with("]: ") && !label.contains(SEP));
    is_path.then(|| (indent, label, t[end..].split(SEP).collect()))
}

/// `true` when a hop is a segment reference (never part of another segment).
fn is_reference(hop: &str) -> bool {
    hop.starts_with("[S") || hop.starts_with("[segment S")
}

/// The characters a run of hops prints.
fn chars(run: &[&str]) -> usize {
    run.iter().map(|h| h.chars().count()).sum::<usize>() + SEP.len() * run.len().saturating_sub(1)
}

/// The lines with every shared run of hops printed once (module doc). Lines
/// that are not path lines are unchanged; the listing is appended at the
/// smallest path indentation less two. Deterministic: candidates are taken
/// in reading order, and segment ids follow the first use in the text.
#[must_use]
pub fn segment(lines: Vec<String>) -> Vec<String> {
    // Every path line's hops, by line index.
    let mut paths: Vec<(usize, Vec<String>)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            path_line(l).map(|(_, _, h)| (i, h.into_iter().map(str::to_string).collect()))
        })
        .collect();
    if paths.len() < 2 {
        return lines;
    }
    // Chosen segments, in the order chosen; references are `\u{0}<k>` until
    // the ids are numbered by first use.
    let mut chosen: Vec<Vec<String>> = Vec::new();
    loop {
        // Distinct paths that hold each candidate run (a path's last hop is
        // never in a run), in reading order of first appearance.
        let mut users: HashMap<Vec<&str>, (usize, usize)> = HashMap::new();
        let mut order: Vec<Vec<&str>> = Vec::new();
        for (p, (_, hops)) in paths.iter().enumerate() {
            let hops: Vec<&str> = hops.iter().map(String::as_str).collect();
            for a in 0..hops.len() {
                for b in (a + MIN_HOPS)..hops.len() {
                    let run = &hops[a..b];
                    if is_reference(run[run.len() - 1]) || run.iter().any(|h| is_hole(h)) {
                        break;
                    }
                    match users.get_mut(run) {
                        Some((n, last)) if *last != p => {
                            *n += 1;
                            *last = p;
                        }
                        Some(_) => {}
                        None => {
                            users.insert(run.to_vec(), (1, p));
                            order.push(run.to_vec());
                        }
                    }
                }
            }
        }
        // The best saving: each use prints a short reference instead of
        // the run; the listing prints it once; the first use names it.
        let mut best: Option<(isize, &Vec<&str>)> = None;
        for run in &order {
            let n = users[run].0;
            if n < 2 {
                continue;
            }
            let c = chars(run) as isize;
            let saving = n as isize * (c - 5) - (c + 8) - 16;
            // The first segment also pays for the listing's header line.
            let threshold = if chosen.is_empty() {
                LISTING.len() as isize
            } else {
                0
            };
            if saving > threshold && best.is_none_or(|(s, _)| saving > s) {
                best = Some((saving, run));
            }
        }
        let Some((_, run)) = best else { break };
        let run: Vec<String> = run.iter().map(|h| (*h).to_string()).collect();
        let k = chosen.len();
        for (_, hops) in &mut paths {
            *hops = replace(hops, &run, &format!("\u{0}{k}"));
        }
        chosen.push(run);
    }
    if chosen.is_empty() {
        return lines;
    }
    // Number the segments by first use in the text.
    let mut id_of: HashMap<usize, usize> = HashMap::new();
    for (_, hops) in &paths {
        for h in hops {
            if let Some(k) = hole(h) {
                let next = id_of.len() + 1;
                id_of.entry(k).or_insert(next);
            }
        }
    }
    let mut seen = vec![false; chosen.len()];
    let indent = paths
        .iter()
        .filter_map(|(i, _)| path_line(&lines[*i]).map(|(n, _, _)| n))
        .min()
        .unwrap_or(2)
        .saturating_sub(2);
    let mut out = lines;
    for (i, hops) in &paths {
        let (n, label, _) = path_line(&out[*i]).expect("a path line");
        let label = label.to_string();
        let text: Vec<String> = hops
            .iter()
            .map(|h| match hole(h) {
                Some(k) if !seen[k] => {
                    seen[k] = true;
                    format!("[segment S{}: {} hops]", id_of[&k], chosen[k].len())
                }
                Some(k) => format!("[S{}]", id_of[&k]),
                None => h.clone(),
            })
            .collect();
        out[*i] = format!("{}{label}{}", " ".repeat(n), text.join(SEP));
    }
    let mut listed: Vec<(usize, &Vec<String>)> = chosen
        .iter()
        .enumerate()
        .map(|(k, r)| (id_of[&k], r))
        .collect();
    listed.sort_by_key(|(id, _)| *id);
    let pad = " ".repeat(indent);
    out.push(format!("{pad}{LISTING}"));
    for (id, run) in listed {
        out.push(format!("{pad}  S{id}: {}", run.join(SEP)));
    }
    out
}

/// A chosen segment's placeholder index.
/// The AL object kinds a hop's display can start with (`Codeunit "X"`).
const OBJECT_KINDS: &[&str] = &[
    "Codeunit",
    "Table",
    "Page",
    "Report",
    "XmlPort",
    "Xmlport",
    "Query",
    "PageExtension",
    "TableExtension",
    "ReportExtension",
    "Enum",
    "EnumExtension",
    "Interface",
    "ControlAddIn",
    "PermissionSet",
    "Profile",
];

/// The location roles a hop's suffix can carry (`(called at f:l)`).
const ROLES: &[&str] = &[" (called at ", " (raised at ", " (operation at "];

/// (tag, display, (role, place) of the location suffix).
type HopParts<'h> = (&'h str, &'h str, Option<(&'h str, &'h str)>);

/// One hop split for elision: (tag prefix like `[event] `, the display,
/// the location suffix's role and place), or `None` for a hop that resets
/// the context (a hub, a segment reference, any other bracketed hop).
fn hop_parts(hop: &str) -> Option<HopParts<'_>> {
    let (tag, rest) = match hop.strip_prefix('[') {
        Some(r) => {
            let close = r.find("] ")?;
            if r[..close].contains(' ') {
                return None;
            }
            hop.split_at(close + 3)
        }
        None => ("", hop),
    };
    let loc = ROLES.iter().find_map(|role| {
        let at = rest.rfind(role)?;
        let place = rest[at + role.len()..].strip_suffix(')')?;
        Some((at, &role[2..role.len() - 1], place))
    });
    Some(match loc {
        Some((at, role, place)) => (tag, &rest[..at], Some((role, place))),
        None => (tag, rest, None),
    })
}

/// The object part of a display (before its last `::` at bracket depth 0,
/// outside quotes), or the whole display when it has none (an object hop).
fn object_of(display: &str) -> (&str, Option<&str>) {
    let (mut depth, mut quoted, mut cut) = (0i32, false, None);
    let b = display.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => quoted = !quoted,
            b'(' | b'[' if !quoted => depth += 1,
            b')' | b']' if !quoted => depth -= 1,
            b':' if !quoted && depth == 0 && b.get(i + 1) == Some(&b':') => {
                cut = Some(i);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    match cut {
        Some(c) => (&display[..c], Some(&display[c + 2..])),
        None => (display, None),
    }
}

/// `true` when a display names an object (`Codeunit "X" (5)`), not a bare
/// member.
fn is_object_display(display: &str) -> bool {
    display
        .split_once(' ')
        .is_some_and(|(k, _)| OBJECT_KINDS.contains(&k))
}

/// Round-2 G33: a path's hops with the repeated object and file elided on
/// consecutive hops: a hop in the same object as the hop before it prints
/// its member only, a location in the same file as the one before it
/// prints `:<line>`. A hub, a segment reference or another bracketed hop
/// resets the context, so the hop after it prints in full.
fn elide_hops(hops: &[&str]) -> Vec<String> {
    let (mut obj, mut file): (Option<String>, Option<String>) = (None, None);
    let mut out = Vec::with_capacity(hops.len());
    for h in hops {
        let Some((tag, display, loc)) = hop_parts(h) else {
            (obj, file) = (None, None);
            out.push((*h).to_string());
            continue;
        };
        let (o, member) = object_of(display);
        let shown = match member {
            Some(m) if obj.as_deref() == Some(o) && !is_object_display(m) => m.to_string(),
            _ => display.to_string(),
        };
        obj = Some(o.to_string());
        let loc_text = match loc {
            Some((role, place)) => {
                let (f, l) = place.rsplit_once(':').unwrap_or((place, ""));
                let text = if file.as_deref() == Some(f) {
                    format!(" ({role} :{l})")
                } else {
                    format!(" ({role} {place})")
                };
                file = Some(f.to_string());
                text
            }
            None => String::new(),
        };
        out.push(format!("{tag}{shown}{loc_text}"));
    }
    out
}

/// [`elide_hops`]' inverse over one line's hops.
fn unelide_hops(hops: &[&str]) -> Vec<String> {
    let (mut obj, mut file): (Option<String>, Option<String>) = (None, None);
    let mut out = Vec::with_capacity(hops.len());
    for h in hops {
        let Some((tag, display, loc)) = hop_parts(h) else {
            (obj, file) = (None, None);
            out.push((*h).to_string());
            continue;
        };
        let full = match (object_of(display), &obj) {
            ((_, None), Some(o)) if !is_object_display(display) => format!("{o}::{display}"),
            _ => display.to_string(),
        };
        obj = Some(object_of(&full).0.to_string());
        let loc_text = match loc {
            Some((role, place)) => {
                let full_place = match (place.strip_prefix(':'), &file) {
                    (Some(l), Some(f)) => format!("{f}:{l}"),
                    _ => place.to_string(),
                };
                file = full_place.rsplit_once(':').map(|(f, _)| f.to_string());
                format!(" ({role} {full_place})")
            }
            None => String::new(),
        };
        out.push(format!("{tag}{full}{loc_text}"));
    }
    out
}

/// [`elide_hops`] over every path line and segment listing line of `lines`
/// (round-2 G33); other lines are unchanged. Run after [`segment`].
#[must_use]
pub fn elide(lines: Vec<String>) -> Vec<String> {
    let mut in_listing = None;
    lines
        .into_iter()
        .map(|l| {
            let indent = l.len() - l.trim_start().len();
            if l.trim() == LISTING {
                in_listing = Some(indent);
                return l;
            }
            if let Some(li) = in_listing {
                match l.trim_start().split_once(": ") {
                    Some((id, hops)) if indent > li && id.starts_with('S') => {
                        let hops: Vec<&str> = hops.split(SEP).collect();
                        return format!(
                            "{}{id}: {}",
                            " ".repeat(indent),
                            elide_hops(&hops).join(SEP)
                        );
                    }
                    _ => in_listing = None,
                }
            }
            match path_line(&l) {
                Some((n, label, hops)) => {
                    format!("{}{label}{}", " ".repeat(n), elide_hops(&hops).join(SEP))
                }
                None => l,
            }
        })
        .collect()
}

fn hole(h: &str) -> Option<usize> {
    h.strip_prefix('\u{0}').and_then(|k| k.parse().ok())
}

fn is_hole(h: &str) -> bool {
    hole(h).is_some()
}

/// `hops` with every use of `run` (left to right, not covering the last
/// hop) replaced by `with`.
fn replace(hops: &[String], run: &[String], with: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(hops.len());
    let mut j = 0;
    while j < hops.len() {
        let end = j + run.len();
        if end < hops.len() && hops[j..end] == *run {
            out.push(with.to_string());
            j = end;
        } else {
            out.push(hops[j].clone());
            j += 1;
        }
    }
    out
}

/// The inverse of [`segment`]: every path line with its segment references
/// replaced by the listed hops, and the listing removed. Text with no
/// listing is returned unchanged. A reference to a segment the listing does
/// not hold is kept as printed (the caller's check then fails on it).
#[must_use]
pub fn expand(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim() == LISTING)
        .unwrap_or(lines.len());
    let indent = lines.get(at).map_or(0, |l| l.len() - l.trim_start().len());
    let mut segments: HashMap<String, String> = HashMap::new();
    let mut end = at + 1;
    while end < lines.len() {
        let l = lines[end];
        let deeper = l.len() - l.trim_start().len() > indent;
        let Some((id, hops)) = l.trim_start().split_once(": ") else {
            break;
        };
        if !deeper || !id.starts_with('S') {
            break;
        }
        // Restored with the whole line below: a segment's first hop may
        // be elided against the hop before its reference.
        segments.insert(id.to_string(), hops.to_string());
        end += 1;
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        if (at..end).contains(&i) {
            continue;
        }
        let Some((n, label, hops)) = path_line(l) else {
            out.push((*l).to_string());
            continue;
        };
        let full: Vec<String> = hops
            .iter()
            .map(|h| {
                let id = h
                    .strip_prefix("[segment ")
                    .and_then(|r| r.split_once(':'))
                    .map(|(id, _)| id)
                    .or_else(|| h.strip_prefix('[').and_then(|r| r.strip_suffix(']')));
                id.and_then(|id| segments.get(id))
                    .cloned()
                    .unwrap_or_else(|| (*h).to_string())
            })
            .collect();
        // Round-2 G33: the elided objects and files back, over the whole
        // expanded line (each hop in the context of the hop before it).
        let full = full.join(SEP);
        let hops: Vec<&str> = full.split(SEP).collect();
        out.push(format!(
            "{}{label}{}",
            " ".repeat(n),
            unelide_hops(&hops).join(SEP)
        ));
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(hops: &str) -> String {
        format!("    path: {hops}")
    }

    /// The witness paths of §7.5's sketch: a shared tail prints once, the
    /// first use names it, later ones refer to it, each keeps its own last
    /// hop, and the listing gives the hops back exactly.
    #[test]
    fn a_shared_run_prints_once_and_expands_back() {
        let tail = "Codeunit \"Auto-Send Runner\" → Codeunit \"Auto-Send Runner\"::OnRun → \
                    Codeunit \"E-Mail Handler\"::DocHandle → Codeunit \"E-Mail Handler\"::RunReport → \
                    Codeunit \"E-Mail Handler\"::RunReportID";
        let lines = vec![
            "  witnesses:".to_string(),
            "    [via-hub] codeunit OnRun: A".to_string(),
            p(&format!(
                "A → A::OnRun → [hub Codeunit.Run of a variable at a.al:4] → {tail} → SetFiles"
            )),
            "    [via-hub] codeunit OnRun: B".to_string(),
            p(&format!("B → B::OnRun → B::Start → {tail} → SetFiles")),
            "  stopped: 1 path".to_string(),
        ];
        let before = lines.join("\n");
        let out = segment(lines);
        let text = out.join("\n");
        assert!(text.contains("[segment S1: 5 hops] → SetFiles"), "{text}");
        assert!(text.contains("B::Start → [S1] → SetFiles"), "{text}");
        assert!(
            text.contains(&format!("  {LISTING}\n    S1: {tail}")),
            "{text}"
        );
        // Round-2 G33: `expand` also restores an elided object (a bare
        // `SetFiles` after a hop in `Codeunit "E-Mail Handler"`).
        let want = before.replace(
            "RunReportID → SetFiles",
            "RunReportID → Codeunit \"E-Mail Handler\"::SetFiles",
        );
        assert_eq!(expand(&text), want);
    }

    /// Round-2 G33: eliding the repeated object and file on consecutive
    /// hops loses nothing — `expand` rebuilds every hop exactly: a member
    /// after a hop in its object, a location in the file of the previous
    /// location, an object hop, a hub (which resets the context, so the hop
    /// after it is never elided), an event tag and a parameter list holding
    /// `::`-free text. Discrimination: see the commit (break `unelide_hops`'
    /// file rebuild).
    #[test]
    fn elided_paths_expand_to_every_hop() {
        let hops = [
            "Codeunit \"Job Queue\" (448)",
            "Codeunit \"Job Queue\" (448)::OnRun (called at dep:Base:src/JQ.al:48)",
            "Codeunit \"Job Queue\" (448)::Handle(Record \"Job Queue Entry\") (called at dep:Base:src/JQ.al:102)",
            "[hub Codeunit.Run of a variable at dep:Base:src/JQ.al:110]",
            "Codeunit \"Job Queue\" (448)::After (called at dep:Base:src/JQ.al:120)",
            "[event] Codeunit Subs::OnAfter (raised at ws:src/S.al:7)",
            "Codeunit Subs::Work (called at ws:src/S.al:9)",
            "Table \"CDO File\".\"Name\"::OnValidate",
        ];
        let line = p(&hops.join(SEP));
        let elided = elide(vec![line.clone()]);
        let text = &elided[0];
        assert!(
            text.contains(" → OnRun (called at dep:Base:src/JQ.al:48) → Handle("),
            "{text}"
        );
        assert!(text.contains("(called at :102)"), "{text}");
        assert!(
            text.contains(
                "] → Codeunit \"Job Queue\" (448)::After (called at dep:Base:src/JQ.al:120)"
            ),
            "the hop after a hub is full: {text}"
        );
        assert!(text.contains(" → Work (called at :9)"), "{text}");
        assert!(text.len() < line.len());
        assert_eq!(expand(text), line);
    }

    /// A path's last hop is never inside a segment, even when another path
    /// is the same run of hops plus more.
    #[test]
    fn a_path_keeps_its_last_hop() {
        let run = "R → One::A → One::B → One::C";
        let lines = vec![
            p(run),
            p(&format!("{run} → One::D")),
            p(&format!("{run} → One::E")),
        ];
        let out = segment(lines.clone());
        assert!(out[0].ends_with("One::C"), "{out:?}");
        assert!(!out[0].contains("[S"), "{out:?}");
        assert_eq!(expand(&out.join("\n")), lines.join("\n"));
    }

    /// Nothing worth sharing: the lines come back unchanged, no listing.
    #[test]
    fn short_or_unshared_paths_are_unchanged() {
        let lines = vec![p("A → B → C"), p("D → E → F → G"), "other".to_string()];
        assert_eq!(segment(lines.clone()), lines);
    }
}
