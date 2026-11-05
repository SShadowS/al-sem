//! Guard enrichment from pinned inputs (agent-tasks design §1.2 M0-3): the
//! enclosing conditions of one site of a revision's sited graph, read from
//! the bytes the revision was built from.
//!
//! The file of the site's routine is taken from the revision's
//! [`WorkspaceInputs`]: a workspace file's pinned text, a source-folder
//! dependency's pinned text, or a package's embedded source, the package
//! re-acquired only if its bytes are the ones the read acquired
//! ([`crate::program::inputs::ClosureEntries::image`]). So the text matches
//! the revision even after the disk has moved. The file is parsed again
//! (`al_syntax::parse`, a few ms), the site is found by its anchor inside
//! the routine's object, and the walk records every `if`/`case`/loop whose
//! body (not whose condition) holds it, outermost first. No assembly need is
//! declared for this: nothing is kept per site, and it works the same under
//! `--low-mem`.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, BlockId, BlockItem, StmtKind};

use crate::program::impact_graph::SitedGraph;
use crate::program::inputs::WorkspaceInputs;
use crate::program::node::{ObjKey, ObjectNodeId};

/// One enclosing condition of a site.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guard {
    /// `if` (the site is in the then-branch), `else`, `case` (in a branch;
    /// `text` is `scrutinee = patterns`), `case-else`, `while`, `repeat`
    /// (`text` is the `until` condition), `for`, `foreach`.
    pub kind: &'static str,
    /// The condition's source text, as written.
    pub text: String,
    /// The controlling statement's line (0-based, as anchors are).
    pub line: u32,
}

/// The guards of site `site` of `g`, read from `inputs` (the revision's
/// pinned read), or why they cannot be read.
pub fn site_guards(
    inputs: &WorkspaceInputs,
    g: &SitedGraph,
    site: u32,
) -> Result<Vec<Guard>, String> {
    let s = g
        .sites()
        .get(site as usize)
        .ok_or_else(|| format!("no site {site}"))?;
    let a = s
        .anchor
        .ok_or_else(|| "the site has no source anchor (no call site makes it)".to_string())?;
    let object = g
        .routine_object(s.from)
        .ok_or_else(|| "the site's caller is not a routine".to_string())?;
    let unit = g.unit(a.unit);
    let texts = pinned_texts(inputs, g, &object, unit)?;
    for text in &texts {
        let file = al_syntax::parse(text);
        let Some(obj) = file.objects.iter().find(|o| declares(o, &object)) else {
            continue;
        };
        let (Some(start), Some(end)) = (
            offset(text, a.start_line, a.start_col),
            offset(text, a.end_line, a.end_col),
        ) else {
            return Err(format!("the anchor is outside {unit}"));
        };
        let mut out = Vec::new();
        for r in &obj.routines {
            if let Some(body) = r.body
                && contains(&file.ir.block(body).origin.byte, start, end)
            {
                walk(&file, text, body, start, end, &mut out);
                return Ok(out);
            }
        }
        return Err(format!("no routine body of {unit} holds the site"));
    }
    Err(format!("{unit}: no pinned file declares the site's object"))
}

/// Every pinned text the site's file may be: the workspace's own file, a
/// source-folder dependency's, or a package's embedded source, by unit.
fn pinned_texts(
    inputs: &WorkspaceInputs,
    g: &SitedGraph,
    object: &ObjectNodeId,
    unit: &str,
) -> Result<Vec<std::sync::Arc<str>>, String> {
    if object.app == g.primary_app() {
        return inputs
            .al_files
            .iter()
            .find(|f| f.rel_posix == unit)
            .map(|f| f.text.clone().map(|t| vec![t]))
            .unwrap_or_else(|| Err(format!("{unit} is not a file of the revision's read")));
    }
    let mut out = Vec::new();
    if let Ok(deps) = &inputs.source_dependencies {
        for d in deps {
            out.extend(
                d.files
                    .iter()
                    .filter(|f| f.virtual_path == unit)
                    .map(|f| f.text.clone()),
            );
        }
    }
    for i in 0..inputs.closure.files.len() {
        // A package the read acquired: its bytes, or a refusal when the file
        // moved since (never the disk's new bytes).
        let Ok(image) = inputs.closure.image(i) else {
            continue;
        };
        if let Ok((files, _)) = crate::snapshot::cache::cached_source(&image) {
            out.extend(
                files
                    .into_iter()
                    .filter(|f| f.virtual_path == unit)
                    .map(|f| f.text),
            );
        }
    }
    Ok(out)
}

fn declares(o: &al_syntax::ir::ObjectDecl, id: &ObjectNodeId) -> bool {
    o.kind == id.kind
        && match &id.key {
            ObjKey::Id(n) => o.id == Some(*n),
            ObjKey::Name(name) => o.name.fold_identifier() == name.fold_identifier(),
        }
}

/// The byte offset of 0-based `(line, col)` (col in bytes), if inside `text`.
fn offset(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let at = start + col as usize;
    (at <= text.len()).then_some(at)
}

fn contains(r: &std::ops::Range<usize>, start: usize, end: usize) -> bool {
    r.start <= start && end <= r.end
}

fn walk(file: &AlFile, text: &str, block: BlockId, start: usize, end: usize, out: &mut Vec<Guard>) {
    let src = |e: al_syntax::ir::ExprId| text[file.ir.expr(e).origin.byte.clone()].to_string();
    let inside = |b: BlockId| contains(&file.ir.block(b).origin.byte, start, end);
    for item in &file.ir.block(block).items {
        match item {
            BlockItem::Preproc(p) => {
                for b in &p.branches {
                    if inside(*b) {
                        walk(file, text, *b, start, end, out);
                        return;
                    }
                }
            }
            BlockItem::Stmt(id) => {
                let st = file.ir.stmt(*id);
                if !contains(&st.origin.byte, start, end) {
                    continue;
                }
                let line = st.origin.start.row;
                match &st.kind {
                    StmtKind::If {
                        cond,
                        then_block,
                        else_block,
                    } => {
                        if inside(*then_block) {
                            out.push(Guard {
                                kind: "if",
                                text: src(*cond),
                                line,
                            });
                            walk(file, text, *then_block, start, end, out);
                        } else if let Some(e) = else_block.filter(|e| inside(*e)) {
                            out.push(Guard {
                                kind: "else",
                                text: src(*cond),
                                line,
                            });
                            walk(file, text, e, start, end, out);
                        }
                    }
                    StmtKind::Case {
                        scrutinee,
                        branches,
                        else_block,
                    } => {
                        if let Some(b) = branches.iter().find(|b| inside(b.body)) {
                            let pats: Vec<String> = b.patterns.iter().map(|p| src(*p)).collect();
                            out.push(Guard {
                                kind: "case",
                                text: format!("{} = {}", src(*scrutinee), pats.join(", ")),
                                line,
                            });
                            walk(file, text, b.body, start, end, out);
                        } else if let Some(e) = else_block.filter(|e| inside(*e)) {
                            out.push(Guard {
                                kind: "case-else",
                                text: src(*scrutinee),
                                line,
                            });
                            walk(file, text, e, start, end, out);
                        }
                    }
                    StmtKind::While { cond, body } if inside(*body) => {
                        out.push(Guard {
                            kind: "while",
                            text: src(*cond),
                            line,
                        });
                        walk(file, text, *body, start, end, out);
                    }
                    StmtKind::Repeat { body, until } if inside(*body) => {
                        out.push(Guard {
                            kind: "repeat",
                            text: src(*until),
                            line,
                        });
                        walk(file, text, *body, start, end, out);
                    }
                    StmtKind::For {
                        var,
                        from,
                        to,
                        body,
                        ..
                    } if inside(*body) => {
                        out.push(Guard {
                            kind: "for",
                            text: format!("{} := {} .. {}", src(*var), src(*from), src(*to)),
                            line,
                        });
                        walk(file, text, *body, start, end, out);
                    }
                    StmtKind::Foreach {
                        var,
                        iterable,
                        body,
                    } if inside(*body) => {
                        out.push(Guard {
                            kind: "foreach",
                            text: format!("{} in {}", src(*var), src(*iterable)),
                            line,
                        });
                        walk(file, text, *body, start, end, out);
                    }
                    StmtKind::With { body, .. }
                    | StmtKind::AssertError(body)
                    | StmtKind::Block(body)
                        if inside(*body) =>
                    {
                        walk(file, text, *body, start, end, out);
                    }
                    StmtKind::Try { body, catch_block } => {
                        if inside(*body) {
                            walk(file, text, *body, start, end, out);
                        } else if let Some(c) = catch_block.filter(|c| inside(*c)) {
                            walk(file, text, c, start, end, out);
                        }
                    }
                    _ => {}
                }
                return;
            }
        }
    }
}
