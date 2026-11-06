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
/// `representative path (…): `) and its hops.
fn path_line(l: &str) -> Option<(usize, &str, Vec<&str>)> {
    let indent = l.len() - l.trim_start().len();
    let t = l.trim_start();
    let end = t.find(": ")? + 2;
    let label = &t[..end];
    let is_path = label == "path: "
        || label == "prefix path: "
        || (label.starts_with("representative path (") && !label.contains(SEP));
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
    let Some(at) = lines.iter().position(|l| l.trim() == LISTING) else {
        return text.to_string();
    };
    let indent = lines[at].len() - lines[at].trim_start().len();
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
        out.push(format!("{}{label}{}", " ".repeat(n), full.join(SEP)));
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
        assert_eq!(expand(&text), before);
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
