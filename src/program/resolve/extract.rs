//! Structured call-site extraction with shape classification — Phase 1 Task 2.
//!
//! Classifies each call expression in every routine body into a [`CalleeShape`]
//! that mirrors L3's `classify_callee` + record-op filter from `ir_walk.rs`.
//! Produces one [`RawSiteV2`] per call site, sorted by `(caller_routine,
//! span.start)`.
//!
//! # Implicit receivers
//! A bare record-op call (`Validate(Field)` in a table trigger, `Modify()`
//! inside `with Cust do`) is a `RecordOp` on the innermost implicit receiver,
//! by L2's rule (`program::body::ir_walk`'s `ImplicitFrame`): the object's
//! implicit `Rec` (`object_has_implicit_rec`), replaced inside a `with` by its
//! receiver when that is a record variable, and by nothing when it is not. A
//! bare name that is also one of the object's own routines is a call (S3.5).
//! Error() handling and any other classification residual vs L2 are MEASURED
//! by the Phase-1 Task-4 site-parity gate.

use std::collections::HashSet;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, BlockId, BlockItem, ExprId, ExprKind, RoutineDecl, StmtKind};

use crate::program::resolve::edge::{CanonicalSpan, SourcePos};

/// Classified callee shape for a call expression, mirroring L3's
/// `classify_callee` + record-op filter.
///
/// # Equality note (`PartialEq`/`Eq`)
///
/// `PartialEq`/`Eq` are implemented MANUALLY below rather than derived, so that
/// `Member`'s `receiver` field (an `ExprId` — Task 2 enabling primitive, see its
/// doc) is excluded from equality/hashing/ordering entirely. `ExprId`s are only
/// stable within the single `AlFile` they were produced from and carry no
/// resolution-affecting information on their own — they must never influence
/// obligation identity, dedup, or golden-comparable output. Comparing two
/// `CalleeShape::Member` values compares only `receiver_text` + `method`,
/// exactly as before this field was added (golden-neutral by construction).
#[derive(Debug, Clone)]
pub enum CalleeShape {
    /// A bare identifier call: `Foo()`.
    Bare { name: String },
    /// A member call on a non-record, non-keyword receiver: `Helper.Process()`.
    Member {
        receiver_text: String,
        method: String,
        /// The parsed receiver expression (Task 2 enabling primitive): the
        /// `ExprId` of the `obj` node classification derives `receiver_text`
        /// from (`file.ir.expr(*object)` in [`classify_call`]). Lets a future
        /// resolver inspect the STRUCTURED receiver (`ExprKind::Call{..}` /
        /// `Member{..}` / …) via `file.ir.expr(id)` instead of re-parsing
        /// `receiver_text`, which is lossy (e.g. cannot recover argument
        /// count/shape, and a `.` inside a string-literal argument would
        /// corrupt a naive text split). `None` only if some future
        /// construction site cannot supply one; the sole current constructor
        /// ([`classify_call`]) always populates `Some`.
        ///
        /// NOT compared by `PartialEq`/`Eq` (see this enum's doc) and NOT part
        /// of any obligation dedup key — purely additive plumbing, consumed by
        /// later resolution steps, never by identity/output today.
        receiver: Option<ExprId>,
    },
    /// An object-run: `Codeunit.Run(...)`, `Page.Run(...)`, `Report.Run(...)`.
    ObjectRun {
        object_kind: String,
        /// Static first argument: the target object name or numeric id, or `None`
        /// when the first argument is a runtime variable (dynamic dispatch).
        /// Derived from `ExprKind::DatabaseReference` only; non-DatabaseReference
        /// arguments produce `None` (mirrors L3's `object_run_callee`).
        target_ref: Option<String>,
        /// `true` when `target_ref` is a name (quoted or bare identifier),
        /// `false` when it is a decimal integer id.
        /// Meaningful only when `target_ref` is `Some`.
        target_is_name: bool,
    },
    /// A record operation on an explicit record-typed receiver: `Rec.SetRange(...)`.
    RecordOp { receiver_text: String, op: String },
    /// A bare `Commit()` call.
    Commit,
    /// Any other call expression that doesn't match a known pattern.
    Unknown,
}

impl PartialEq for CalleeShape {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CalleeShape::Bare { name: a }, CalleeShape::Bare { name: b }) => a == b,
            (
                CalleeShape::Member {
                    receiver_text: rt1,
                    method: m1,
                    // Deliberately excluded from equality — see the enum doc.
                    receiver: _,
                },
                CalleeShape::Member {
                    receiver_text: rt2,
                    method: m2,
                    receiver: _,
                },
            ) => rt1 == rt2 && m1 == m2,
            (
                CalleeShape::ObjectRun {
                    object_kind: ok1,
                    target_ref: tr1,
                    target_is_name: tn1,
                },
                CalleeShape::ObjectRun {
                    object_kind: ok2,
                    target_ref: tr2,
                    target_is_name: tn2,
                },
            ) => ok1 == ok2 && tr1 == tr2 && tn1 == tn2,
            (
                CalleeShape::RecordOp {
                    receiver_text: rt1,
                    op: op1,
                },
                CalleeShape::RecordOp {
                    receiver_text: rt2,
                    op: op2,
                },
            ) => rt1 == rt2 && op1 == op2,
            (CalleeShape::Commit, CalleeShape::Commit) => true,
            (CalleeShape::Unknown, CalleeShape::Unknown) => true,
            _ => false,
        }
    }
}

impl Eq for CalleeShape {}

/// One classified call site extracted from a routine body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSiteV2 {
    /// The enclosing routine's name, lowercased.
    pub caller_routine: String,
    /// Classified callee shape.
    pub shape: CalleeShape,
    /// Number of arguments at the call site.
    pub arity: usize,
    /// Source span of the whole call expression (callee + argument list).
    pub span: CanonicalSpan,
    /// Raw source text of the callee (function) expression — e.g. `"Foo"` or
    /// `"Rec.Insert"`. Does NOT include the argument list. Used to compute
    /// `callee_fp` for the site-parity differential harness (must match L3's
    /// `PCallSite::callee_text` derivation, which is also the raw function-
    /// expression bytes).
    pub callee_text: String,
    /// Tri-state `with`-scope guard — see [`WithState`]. Consumed by
    /// `resolve_bare`'s Step 3 (beyond-1B.3b Task 3): a bare call is only
    /// eligible for implicit-`Rec` fallback when this is `NoWithProven`.
    pub with_state: WithState,
    /// The call's argument expression ids, in source order (argtype-dispatch-
    /// and-page-catalog plan, Task 2). Consumed by `resolve_call_site_
    /// obligation` to build the [`crate::program::resolve::arg_dispatch::
    /// ArgDispatchInfo`] list a fail-closed overload-dispatch pick needs.
    pub args: Vec<ExprId>,
    /// `true` for a bare `M` or `X.M` read as a VALUE (an argument, an operand,
    /// a condition, an assigned or returned value). AL lets a zero-argument
    /// call drop its `()`, so such a read MAY be a call; syntax cannot tell it
    /// from a variable or field read. The resolver keeps the site only when it
    /// resolves to a routine (`full.rs`, `resolve_file_obligations`).
    pub parenless: bool,
    /// The site's expression: the call, or for a parens-less site the read.
    pub expr: ExprId,
}

/// Tri-state guard for whether a call site sits lexically inside a `with X do`
/// block — consumed by `resolve_bare`'s Step 3 (implicit-`Rec` bare-call
/// fallback, beyond-1B.3b Task 3). AL's `with` rebinds the meaning of a bare
/// identifier to the `with`-receiver's members; the implicit-`Rec` fallback
/// must NEVER fire inside a `with` it cannot see, since running it there could
/// synthesize a Route the compiler would actually attribute to the `with`
/// receiver instead — a false `Source` edge, the cardinal sin.
///
/// # Two independent signals, ANDed for `NoWithProven`
///
/// [`walk_stmt_v2`] is an EXHAUSTIVE match over every [`StmtKind`] variant (no
/// wildcard `_` arm — the compiler enforces this stays exhaustive as the IR
/// grows), so the AST-based `with`-depth this module tracks while walking is
/// structurally sound for any call site it actually visits: a site is
/// `InsideWith` if and only if the walk passed through a `StmtKind::With`
/// body to reach it. That alone would already be a precise per-site proof.
///
/// This project's tree-sitter-al grammar has nonetheless had real history of
/// silent field/shape surprises (see `CLAUDE.md`'s "tree-sitter-al grammar
/// issues" notes), so Step 3's guard does not rely on the AST signal alone: a
/// cheap, redundant whole-routine raw-text scan for a standalone `with` token
/// ([`routine_has_with_token`]) is combined conjunctively. `NoWithProven` only
/// when BOTH signals agree there is no `with` in play; if the raw scan finds a
/// `with` token anywhere in the routine's source while the AST places the
/// current site at depth 0, that DISAGREEMENT is treated as `Unknown` (skip)
/// rather than trusted — a false positive (over-skip) is always safe, a false
/// negative is fatal.
///
/// Task 4 (receiver-closure-and-arg-increments plan) made the raw-text scan
/// comment/string/quoted-identifier AWARE (see [`routine_has_with_token`]'s
/// doc) — a `with` token appearing only in a comment (the exact shape that
/// caused a real CDO false-`Unknown`: `UseContiniaAuthorization`'s leading
/// comment, see the CDO harness ratchet history) no longer disagrees with the
/// AST signal. The two-signal ANDed design itself is UNCHANGED — only the
/// text signal's precision improved; an uncertain (unterminated) comment/
/// string/quoted-identifier still conservatively counts as a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithState {
    /// Both the AST-depth walk and the raw-text scan agree: this call site is
    /// NOT inside any `with` block anywhere in its enclosing routine.
    /// `resolve_bare` Step 3 may run.
    NoWithProven,
    /// The AST-depth walk places this call site inside at least one
    /// `StmtKind::With` body. `resolve_bare` Step 3 must skip.
    InsideWith,
    /// The AST says depth 0 (not inside a `with`) but the routine's raw
    /// source text still contains a `with` token — the two signals disagree,
    /// so the AST placement is not trusted alone. `resolve_bare` Step 3 must
    /// skip (fail closed).
    Unknown,
}

/// Returns `true` if the raw AL type string `ty` denotes a record type.
///
/// Mirrors `ir_walk.rs::is_record_receiver_ty`: the string must start with
/// `"record"` (case-insensitive), INCLUSIVE of `RecordRef`. This ensures that
/// a `RecordRef`-typed var's record-op calls classify as `RecordOp` (matching L3,
/// which makes no PCallSite for them).
fn is_record_ty(ty: &str) -> bool {
    ty.trim().to_ascii_lowercase().starts_with("record")
}

/// Compute the set of lowercased record-receiver variable names in scope for a
/// routine.
///
/// Always includes `"rec"` and `"xrec"` (the AL implicit record vars). Any param
/// or local whose declared type is a Record type (via [`is_record_ty`]) is also
/// included.
///
/// Object-level globals are NOT included here — pass them via `object_globals` in
/// [`extract_sites`].
pub fn routine_rvars(routine: &RoutineDecl) -> HashSet<String> {
    let mut rvars = HashSet::new();
    rvars.insert("rec".to_string());
    rvars.insert("xrec".to_string());
    for p in &routine.params {
        if p.ty.as_deref().map(is_record_ty).unwrap_or(false) {
            rvars.insert(p.name.fold_identifier());
        }
    }
    for v in &routine.locals {
        if v.ty.as_deref().map(is_record_ty).unwrap_or(false) {
            rvars.insert(v.name.fold_identifier());
        }
    }
    rvars
}

/// Convert a byte offset into a 0-based `(line, col)` source position by
/// counting newlines in the prefix `src[..byte]`. Mirrors `extract_min.rs`.
fn byte_to_pos(src: &str, byte: usize) -> SourcePos {
    let byte = byte.min(src.len());
    let prefix = &src[..byte];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let col = match prefix.rfind('\n') {
        Some(nl) => (byte - nl - 1) as u32,
        None => byte as u32,
    };
    SourcePos { line, col }
}

/// Strip one layer of surrounding double-quotes or single-quotes (mirrors L2's
/// `strip_quote_chars`).
fn strip_quote_chars(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Object-run object kind for a `keyword_identifier` receiver (mirrors L2's
/// `object_run_kind`).
fn object_run_kind(text: &str) -> Option<&'static str> {
    match text.trim().to_ascii_lowercase().as_str() {
        "codeunit" => Some("Codeunit"),
        "page" => Some("Page"),
        "report" => Some("Report"),
        "xmlport" => Some("XmlPort"),
        _ => None,
    }
}

/// Extract a statically-named target from `args`' first element, when it is
/// an `ExprKind::DatabaseReference` (`Page::"X"` / `Report::42` / …).
/// Returns `Some((target_ref, target_is_name))`; `None` when the first
/// argument is absent or not a `DatabaseReference` (a runtime
/// variable/expression — dynamic dispatch, not statically named). Mirrors
/// L3's `object_run_callee` target extraction.
///
/// Factored out of `classify_call`'s `ObjectRun` check (Check 2 below) so the
/// SAME extraction backs both that check AND the T0.3 builtin-dispatch audit
/// (`program::resolve::full`'s `builtin_dispatch_finding`, which needs the
/// identical logic for a `Page.RunModal(Page::"X")`-shaped call that Check 2
/// does NOT classify as `ObjectRun` — `method_lc == "run"`-only, see
/// `member_catalog::ENTRY_DISPATCH_BUILTIN_IDS`'s doc) — never drift between
/// the two.
pub(crate) fn static_database_reference_target(
    file: &AlFile,
    args: &[ExprId],
) -> Option<(String, bool)> {
    let &first_arg = args.first()?;
    let ExprKind::DatabaseReference(text) = &file.ir.expr(first_arg).kind else {
        return None;
    };
    let (_, tn) = text.split_once("::")?;
    let tn = tn.trim();
    if tn.starts_with('"') {
        // Quoted name: strip surrounding quotes.
        Some((strip_quote_chars(tn), true))
    } else if tn.parse::<i64>().is_ok() {
        // Decimal integer id.
        Some((tn.to_string(), false))
    } else {
        // Bare (unquoted) name.
        Some((tn.to_string(), true))
    }
}

/// Classify a single call's callee into a [`CalleeShape`].
///
/// Classification precedence (mirrors L3's `classify_callee` + record-op filter
/// from `ir_walk.rs`):
///
/// 1. `Member` with `Identifier`/`QuotedIdentifier` receiver in `rvars` AND
///    method a record op ([`crate::record_ops::record_op_type`], the table L2
///    also reads) → `RecordOp`.
/// 2. `Member` with `keyword_identifier` receiver (`codeunit`/`page`/`report`)
///    AND method `"run"` (any kind) OR `"runmodal"` (Page/Report only —
///    Codeunit has no RunModal member) → `ObjectRun`.
/// 3. Any other `Member` → `Member`.
/// 4. Bare `Identifier("commit")` / `QuotedIdentifier("commit")` → `Commit`.
/// 5. A bare record op on a record implicit receiver, not named like one of
///    the object's routines → `RecordOp` (see the module doc).
/// 6. Any other bare `Identifier` / `QuotedIdentifier` → `Bare`.
/// 7. Everything else → `Unknown`.
fn classify_call(
    file: &AlFile,
    src: &str,
    function: ExprId,
    args: &[ExprId],
    rvars: &HashSet<String>,
    ctx: WithCtx,
) -> CalleeShape {
    let fe = file.ir.expr(function);
    match &fe.kind {
        ExprKind::Member { object, member, .. } => {
            let obj = file.ir.expr(*object);
            // Strip quotes + lowercase for matching.
            let method_lc = strip_quote_chars(member).fold_identifier();

            // --- Check 1: RecordOp ------------------------------------------------
            // Receiver must be a simple Identifier/QuotedIdentifier in rvars AND
            // method must be an L2 record op -- the SAME table, so the two engines
            // cannot drift (a hand copy here once lacked `rename`, #9).
            let recv_lc = match &obj.kind {
                ExprKind::Identifier(r) | ExprKind::QuotedIdentifier(r) => {
                    Some(r.fold_identifier())
                }
                _ => None,
            };
            if let Some(ref r_lc) = recv_lc
                && rvars.contains(r_lc)
                && crate::record_ops::record_op_type(&method_lc).is_some()
            {
                let receiver_text = src[obj.origin.byte.clone()].to_string();
                return CalleeShape::RecordOp {
                    receiver_text,
                    op: method_lc,
                };
            }

            // --- Check 2: ObjectRun -----------------------------------------------
            // Mirrors L2's `object_run_callee`: receiver is `keyword_identifier`
            // (Codeunit/Page/Report) AND method is "run" — OR "runmodal" for
            // Page/Report only (T1.3, deep-review-remediation plan: Codeunit has
            // no RunModal member at all — MS Learn documents it only on
            // Page/Report — so `object_kind == "Codeunit"` must never take this
            // branch on "runmodal"; see `member_catalog::ENTRY_DISPATCH_BUILTIN_
            // IDS`'s doc for the classifier-gap analysis this closes).
            // Target extraction: only `ExprKind::DatabaseReference` arguments carry
            // a static target; all other argument kinds (variables, integer literals
            // NOT wrapped in DatabaseReference) produce `target_ref = None` (dynamic
            // dispatch — mirrors L3's behaviour).
            if obj.origin.kind_text == "keyword_identifier" {
                let obj_text = &src[obj.origin.byte.clone()];
                if let Some(okind) = object_run_kind(obj_text)
                    && (method_lc == "run"
                    || (method_lc == "runmodal" && matches!(okind, "Page" | "Report"))
                    // Static `XmlPort.Import/Export(XmlPort::X, ...)` run X too.
                    || (okind == "XmlPort" && matches!(method_lc.as_str(), "import" | "export")))
                {
                    let (target_ref, target_is_name) =
                        match static_database_reference_target(file, args) {
                            Some((name, is_name)) => (Some(name), is_name),
                            None => (None, false),
                        };
                    return CalleeShape::ObjectRun {
                        object_kind: okind.to_string(),
                        target_ref,
                        target_is_name,
                    };
                }
            }

            // --- Check 3: General Member ------------------------------------------
            let receiver_text = src[obj.origin.byte.clone()].to_string();
            let method = strip_quote_chars(member);
            CalleeShape::Member {
                receiver_text,
                method,
                receiver: Some(*object),
            }
        }

        // QuotedIdentifier stores the already-unquoted name (lowerer strips quotes).
        ExprKind::Identifier(name) | ExprKind::QuotedIdentifier(name) => {
            if name.eq_fold_identifier("commit") {
                return CalleeShape::Commit;
            }
            let op = name.fold_identifier();
            let receiver_text = match ctx.implicit {
                ImplicitRecv::NotRecord => None,
                ImplicitRecv::Rec => Some("Rec".to_string()),
                ImplicitRecv::With(r) => {
                    Some(src[file.ir.expr(r).origin.byte.clone()].trim().to_string())
                }
            };
            match receiver_text {
                Some(receiver_text)
                    if crate::record_ops::record_op_type(&op).is_some()
                        && !file.objects[ctx.object_idx]
                            .routines
                            .iter()
                            .any(|r| r.name.eq_fold_identifier(&op)) =>
                {
                    CalleeShape::RecordOp { receiver_text, op }
                }
                _ => CalleeShape::Bare { name: name.clone() },
            }
        }

        _ => CalleeShape::Unknown,
    }
}

/// Threaded through the whole per-routine call-site walk to compute each
/// site's [`WithState`] — see that type's doc for the two-signal ANDed
/// soundness argument.
#[derive(Debug, Clone, Copy)]
struct WithCtx {
    /// AST `with`-nesting depth at the current walk position (0 = not
    /// currently inside any `StmtKind::With` body).
    depth: u32,
    /// Whole-routine raw-text scan result, computed ONCE before the walk
    /// starts (see [`routine_has_with_token`]): `true` when the routine's
    /// source text contains a standalone `with` token anywhere OUTSIDE a
    /// comment/string-literal/quoted-identifier (Task 4, receiver-closure-
    /// and-arg-increments plan: the scan is comment/string/quoted-identifier
    /// AWARE — see that function's doc), OR when a comment/string/quoted-
    /// identifier is itself UNTERMINATED (uncertain -> conservative
    /// over-approximation, never a false negative).
    scan_hit: bool,
    /// The innermost implicit receiver at the current walk position.
    implicit: ImplicitRecv,
    /// Index of the routine's object in `file.objects`.
    object_idx: usize,
}

/// The innermost implicit receiver of a bare call (L2's `ImplicitFrame`).
#[derive(Debug, Clone, Copy)]
enum ImplicitRecv {
    /// None, or one that is not a record (a `with` on a non-record).
    NotRecord,
    /// The object's implicit `Rec`.
    Rec,
    /// The receiver of the innermost `with`, a record variable.
    With(ExprId),
}

impl WithCtx {
    /// The context at the start of `file.objects[object_idx]`'s `routine`.
    fn for_routine(src: &str, file: &AlFile, object_idx: usize, routine: &RoutineDecl) -> WithCtx {
        let rec = crate::program::body::ir_walk::object_has_implicit_rec(&file.objects[object_idx]);
        WithCtx {
            depth: 0,
            scan_hit: routine_has_with_token(src, routine.origin.byte.clone()),
            implicit: if rec {
                ImplicitRecv::Rec
            } else {
                ImplicitRecv::NotRecord
            },
            object_idx,
        }
    }

    /// Depth incremented on entry to a `StmtKind::With` body, whose implicit
    /// receiver is `receiver` when that is a record variable.
    fn entered_with(self, file: &AlFile, receiver: ExprId, rvars: &HashSet<String>) -> WithCtx {
        let is_record = match &file.ir.expr(receiver).kind {
            ExprKind::Identifier(x) | ExprKind::QuotedIdentifier(x) => {
                rvars.contains(&x.fold_identifier())
            }
            _ => false,
        };
        WithCtx {
            depth: self.depth + 1,
            implicit: if is_record {
                ImplicitRecv::With(receiver)
            } else {
                ImplicitRecv::NotRecord
            },
            ..self
        }
    }

    /// Combine both signals into the [`WithState`] a call site at this
    /// context is tagged with.
    fn state(self) -> WithState {
        if self.depth > 0 {
            WithState::InsideWith
        } else if self.scan_hit {
            WithState::Unknown
        } else {
            WithState::NoWithProven
        }
    }
}

/// `true` if an AL identifier-constituent byte (letter/digit/underscore) —
/// used by [`routine_has_with_token`] to enforce word boundaries so `Within`/
/// `WidthValue`/etc. never false-match the standalone `with` keyword.
fn is_al_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whole-routine raw-text scan (ASCII case-insensitive, word-bounded) for a
/// standalone `with` token anywhere in `routine_span`'s source text — the
/// redundant safety net [`WithCtx::state`] ANDs with the AST-depth signal (see
/// [`WithState`]'s doc for why).
///
/// # Comment/string/quoted-identifier aware (Task 4, receiver-closure-and-
/// arg-increments plan — the round-2 closer, BINDING)
///
/// The `AlFile`/`Ir` layer this module otherwise consumes drops comments as
/// trivia at lowering time (`schema::kind_policy`'s Trivia classification —
/// they never reach the IR at all), and `AlFile` retains no reference to the
/// raw tree-sitter tree either (see `al_syntax::ir::AlFile`'s fields) — so
/// there is no lexer/token layer available at this point; this function is a
/// hand-rolled LEXER-LITE over the raw bytes, excluding exactly the four AL
/// lexical shapes that can hide a "with" substring without it being the real
/// keyword:
/// - `// ...` line comments (to end of line).
/// - `/* ... */` block comments — NON-NESTED (AL, like C#, does not nest
///   block comments; the first `*/` closes it).
/// - `'...'` string literals, with AL's `''` doubled-quote escape for a
///   literal `'` inside the string.
/// - `"..."` quoted identifiers, with the same `""` doubled-quote escape for
///   a literal `"` inside the name.
///
/// A REAL `with` keyword outside all four of these shapes still hits, exactly
/// as before. **Uncertain → conservative**: an UNTERMINATED comment/string/
/// quoted-identifier (runs off the end of `routine_span` without its closing
/// delimiter — malformed input, should not happen for a real parsed routine
/// span, but this function must not silently under-detect if it does) returns
/// `true` immediately — the same "a false positive is always safe, a false
/// negative is fatal" direction the original comment-blind scan relied on,
/// now scoped to genuinely uncertain cases only rather than every comment.
///
/// `routine_span` is expected to be a valid UTF-8 char-boundary byte range (a
/// tree-sitter node's byte span, as used throughout this module); the scan is
/// byte-wise (ASCII-focused, mirrors [`is_al_ident_byte`]) — safe over UTF-8
/// text since none of the delimiter/keyword bytes checked here ever appear as
/// a continuation byte of a multi-byte UTF-8 sequence.
fn routine_has_with_token(src: &str, routine_span: std::ops::Range<usize>) -> bool {
    let text = &src[routine_span];
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // `// ...` line comment — skip to end of line (or end of text).
        if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // `/* ... */` block comment — non-nested; unterminated -> conservative.
        if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            let mut j = i + 2;
            let mut closed = false;
            while j + 1 < len {
                if bytes[j] == b'*' && bytes[j + 1] == b'/' {
                    closed = true;
                    break;
                }
                j += 1;
            }
            if !closed {
                return true; // unterminated -> uncertain -> conservative
            }
            i = j + 2;
            continue;
        }
        // `'...'` string literal — AL `''` doubled-quote escape.
        if bytes[i] == b'\'' {
            i += 1;
            loop {
                if i >= len {
                    return true; // unterminated -> uncertain -> conservative
                }
                if bytes[i] == b'\'' {
                    if i + 1 < len && bytes[i + 1] == b'\'' {
                        i += 2; // escaped quote, string continues
                        continue;
                    }
                    i += 1; // real closing quote
                    break;
                }
                i += 1;
            }
            continue;
        }
        // `"..."` quoted identifier — same `""` doubled-quote escape.
        if bytes[i] == b'"' {
            i += 1;
            loop {
                if i >= len {
                    return true; // unterminated -> uncertain -> conservative
                }
                if bytes[i] == b'"' {
                    if i + 1 < len && bytes[i + 1] == b'"' {
                        i += 2; // escaped quote, identifier continues
                        continue;
                    }
                    i += 1; // real closing quote
                    break;
                }
                i += 1;
            }
            continue;
        }
        // Real code text: the standalone `with` keyword check, unchanged.
        if i + 4 <= len && bytes[i..i + 4].eq_ignore_ascii_case(b"with") {
            let before_ok = i == 0 || !is_al_ident_byte(bytes[i - 1]);
            let after_ok = i + 4 == len || !is_al_ident_byte(bytes[i + 4]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// [`collect_calls_v2`] for an expression read as a VALUE: a bare `M` or
/// `X.M` there is also a candidate parens-less call ([`RawSiteV2::parenless`]).
/// A callee or a member's receiver is not a value position: those go straight
/// to [`collect_calls_v2`].
#[allow(clippy::too_many_arguments)]
fn collect_value_v2(
    file: &AlFile,
    src: &str,
    eid: ExprId,
    unit: &str,
    caller: &str,
    rvars: &HashSet<String>,
    ctx: WithCtx,
    out: &mut Vec<RawSiteV2>,
) {
    let e = file.ir.expr(eid);
    if matches!(
        e.kind,
        ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_) | ExprKind::Member { .. }
    ) {
        out.push(RawSiteV2 {
            caller_routine: caller.to_string(),
            shape: classify_call(file, src, eid, &[], rvars, ctx),
            arity: 0,
            span: CanonicalSpan {
                unit: unit.to_string(),
                start: byte_to_pos(src, e.origin.byte.start),
                end: byte_to_pos(src, e.origin.byte.end),
            },
            callee_text: src[e.origin.byte.clone()].to_string(),
            with_state: ctx.state(),
            args: Vec::new(),
            parenless: true,
            expr: eid,
        });
    }
    collect_calls_v2(file, src, eid, unit, caller, rvars, ctx, out);
}

/// Recursively collect every [`RawSiteV2`] reachable from `eid`, including
/// calls nested inside arguments or chained receivers.
#[allow(clippy::too_many_arguments)]
fn collect_calls_v2(
    file: &AlFile,
    src: &str,
    eid: ExprId,
    unit: &str,
    caller: &str,
    rvars: &HashSet<String>,
    ctx: WithCtx,
    out: &mut Vec<RawSiteV2>,
) {
    let e = file.ir.expr(eid);
    match &e.kind {
        ExprKind::Call { function, args } => {
            let fn_id = *function;
            let arg_ids = args.to_vec();

            // Classify and emit this call site.
            let shape = classify_call(file, src, fn_id, &arg_ids, rvars, ctx);
            // callee_text = raw source bytes of the function expression (not the
            // arg list).  Mirrors extract_min.rs and L3's ir_walk classify_callee
            // so callee_fp agrees between the two sides of the harness.
            let callee_text = src[file.ir.expr(fn_id).origin.byte.clone()].to_string();
            let span = CanonicalSpan {
                unit: unit.to_string(),
                start: byte_to_pos(src, e.origin.byte.start),
                end: byte_to_pos(src, e.origin.byte.end),
            };
            out.push(RawSiteV2 {
                caller_routine: caller.to_string(),
                shape,
                arity: arg_ids.len(),
                span,
                callee_text,
                with_state: ctx.state(),
                args: arg_ids.clone(),
                parenless: false,
                expr: eid,
            });

            // Recurse: function expression (catches chained calls), then args.
            collect_calls_v2(file, src, fn_id, unit, caller, rvars, ctx, out);
            for a in arg_ids {
                collect_value_v2(file, src, a, unit, caller, rvars, ctx, out);
            }
        }
        ExprKind::Member { object, .. } => {
            let obj = *object;
            collect_calls_v2(file, src, obj, unit, caller, rvars, ctx, out);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            let (l, r) = (*lhs, *rhs);
            collect_value_v2(file, src, l, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, r, unit, caller, rvars, ctx, out);
        }
        ExprKind::Unary { operand, .. } => {
            let op = *operand;
            collect_value_v2(file, src, op, unit, caller, rvars, ctx, out);
        }
        ExprKind::Parenthesized(x) => {
            let x = *x;
            collect_value_v2(file, src, x, unit, caller, rvars, ctx, out);
        }
        ExprKind::Index { base, index } => {
            let (b, i) = (*base, *index);
            collect_calls_v2(file, src, b, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, i, unit, caller, rvars, ctx, out);
        }
        ExprKind::RangeExpr { start, end } => {
            let (s, e2) = (*start, *end);
            collect_value_v2(file, src, s, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, e2, unit, caller, rvars, ctx, out);
        }
        ExprKind::QualifiedEnum { enum_type, .. } => {
            let et = *enum_type;
            collect_calls_v2(file, src, et, unit, caller, rvars, ctx, out);
        }
        ExprKind::Ternary { .. } | ExprKind::TypeOp { .. } | ExprKind::List(_) => {
            for c in e.kind.children() {
                collect_value_v2(file, src, c, unit, caller, rvars, ctx, out);
            }
        }
        // Identifier / QuotedIdentifier / Literal / DatabaseReference / Unknown:
        // no nested calls.
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_block_v2(
    file: &AlFile,
    src: &str,
    bid: BlockId,
    unit: &str,
    caller: &str,
    rvars: &HashSet<String>,
    ctx: WithCtx,
    out: &mut Vec<RawSiteV2>,
) {
    for item in &file.ir.block(bid).items {
        match item {
            BlockItem::Stmt(sid) => {
                let st = file.ir.stmt(*sid);
                walk_stmt_v2(file, src, &st.kind, unit, caller, rvars, ctx, out);
            }
            BlockItem::Preproc(g) => {
                for b in &g.branches {
                    walk_block_v2(file, src, *b, unit, caller, rvars, ctx, out);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_stmt_v2(
    file: &AlFile,
    src: &str,
    kind: &StmtKind,
    unit: &str,
    caller: &str,
    rvars: &HashSet<String>,
    ctx: WithCtx,
    out: &mut Vec<RawSiteV2>,
) {
    match kind {
        StmtKind::Assignment { target, value, .. } => {
            collect_calls_v2(file, src, *target, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, *value, unit, caller, rvars, ctx, out);
        }
        StmtKind::Call(eid) => {
            collect_calls_v2(file, src, *eid, unit, caller, rvars, ctx, out);
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_value_v2(file, src, *cond, unit, caller, rvars, ctx, out);
            walk_block_v2(file, src, *then_block, unit, caller, rvars, ctx, out);
            if let Some(b) = else_block {
                walk_block_v2(file, src, *b, unit, caller, rvars, ctx, out);
            }
        }
        StmtKind::Case {
            scrutinee,
            branches,
            else_block,
        } => {
            collect_value_v2(file, src, *scrutinee, unit, caller, rvars, ctx, out);
            for br in branches {
                for &p in &br.patterns {
                    collect_value_v2(file, src, p, unit, caller, rvars, ctx, out);
                }
                walk_block_v2(file, src, br.body, unit, caller, rvars, ctx, out);
            }
            if let Some(b) = else_block {
                walk_block_v2(file, src, *b, unit, caller, rvars, ctx, out);
            }
        }
        StmtKind::While { cond, body } => {
            collect_value_v2(file, src, *cond, unit, caller, rvars, ctx, out);
            walk_block_v2(file, src, *body, unit, caller, rvars, ctx, out);
        }
        StmtKind::Repeat { body, until } => {
            walk_block_v2(file, src, *body, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, *until, unit, caller, rvars, ctx, out);
        }
        StmtKind::For {
            var,
            from,
            to,
            body,
            ..
        } => {
            collect_calls_v2(file, src, *var, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, *from, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, *to, unit, caller, rvars, ctx, out);
            walk_block_v2(file, src, *body, unit, caller, rvars, ctx, out);
        }
        StmtKind::Foreach {
            var,
            iterable,
            body,
        } => {
            collect_calls_v2(file, src, *var, unit, caller, rvars, ctx, out);
            collect_value_v2(file, src, *iterable, unit, caller, rvars, ctx, out);
            walk_block_v2(file, src, *body, unit, caller, rvars, ctx, out);
        }
        StmtKind::With { receiver, body } => {
            // The receiver expression itself is evaluated OUTSIDE the `with`
            // body (`with SomeFunc() do` calls `SomeFunc()` in the enclosing
            // scope), so it keeps the CURRENT (un-incremented) depth; only
            // the body walk enters the `with`.
            collect_calls_v2(file, src, *receiver, unit, caller, rvars, ctx, out);
            walk_block_v2(
                file,
                src,
                *body,
                unit,
                caller,
                rvars,
                ctx.entered_with(file, *receiver, rvars),
                out,
            );
        }
        StmtKind::Try { body, catch_block } => {
            walk_block_v2(file, src, *body, unit, caller, rvars, ctx, out);
            if let Some(c) = catch_block {
                walk_block_v2(file, src, *c, unit, caller, rvars, ctx, out);
            }
        }
        StmtKind::AssertError(body) => {
            walk_block_v2(file, src, *body, unit, caller, rvars, ctx, out);
        }
        StmtKind::Exit(x) => {
            if let Some(e) = x {
                collect_value_v2(file, src, *e, unit, caller, rvars, ctx, out);
            }
        }
        StmtKind::Block(b) => {
            walk_block_v2(file, src, *b, unit, caller, rvars, ctx, out);
        }
        StmtKind::Break | StmtKind::Continue | StmtKind::Unknown => {}
    }
}

/// Walk every routine body in `file` and return one [`RawSiteV2`] per call
/// expression, classified into a [`CalleeShape`].
///
/// `src` is the original AL source text; `unit` names the file (e.g. `"C.al"`);
/// `object_globals` is the set of lowercased record-typed global variable names
/// from the enclosing object (the caller is responsible for filtering to
/// record-typed globals before passing in). Per routine, `rvars = routine_rvars(routine)
/// ∪ object_globals`. The result is sorted by `(caller_routine, span.start)`.
///
/// **Multi-object limitation:** when a file contains more than one object and
/// two objects share a routine name, the returned list will contain sites from
/// BOTH routines under the same `caller_routine` label.  Callers that need to
/// attribute sites to a single object should use [`extract_sites_for_object`]
/// instead.
pub fn extract_sites(
    file: &AlFile,
    src: &str,
    unit: &str,
    object_globals: &HashSet<String>,
) -> Vec<RawSiteV2> {
    let mut out = Vec::new();
    for (obj_idx, obj) in file.objects.iter().enumerate() {
        for routine in &obj.routines {
            if let Some(body) = routine.body {
                let caller = routine.name.fold_identifier();
                let mut rvars = routine_rvars(routine);
                rvars.extend(object_globals.iter().cloned());
                let ctx = WithCtx::for_routine(src, file, obj_idx, routine);
                walk_block_v2(file, src, body, unit, &caller, &rvars, ctx, &mut out);
            }
        }
    }
    out.sort_by(|a, b| {
        a.caller_routine
            .cmp(&b.caller_routine)
            .then_with(|| a.span.start.cmp(&b.span.start))
    });
    out
}

/// Walk only the routines of `file.objects[obj_idx]` and return one
/// [`RawSiteV2`] per call expression.
///
/// Unlike [`extract_sites`] (which processes ALL objects in the file),
/// this variant is scoped to a single object so that sites are unambiguously
/// attributed to that object even when multiple objects in the same file share
/// routine names.
///
/// `object_globals` should contain only the record-typed global variable names
/// declared in `file.objects[obj_idx]`.  The result is sorted by
/// `(caller_routine, span.start)`.
///
/// Returns an empty `Vec` if `obj_idx` is out of range.
pub fn extract_sites_for_object(
    file: &AlFile,
    src: &str,
    unit: &str,
    object_globals: &HashSet<String>,
    obj_idx: usize,
) -> Vec<RawSiteV2> {
    let Some(obj) = file.objects.get(obj_idx) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for routine in &obj.routines {
        if let Some(body) = routine.body {
            let caller = routine.name.fold_identifier();
            let mut rvars = routine_rvars(routine);
            rvars.extend(object_globals.iter().cloned());
            let ctx = WithCtx::for_routine(src, file, obj_idx, routine);
            walk_block_v2(file, src, body, unit, &caller, &rvars, ctx, &mut out);
        }
    }
    out.sort_by(|a, b| {
        a.caller_routine
            .cmp(&b.caller_routine)
            .then_with(|| a.span.start.cmp(&b.span.start))
    });
    out
}

/// Walk only `file.objects[obj_idx].routines[routine_idx]` and return one
/// [`RawSiteV2`] per call expression in that single routine body.
///
/// This is the per-routine companion to [`extract_sites_for_object`].  Use it
/// when iterating over `obj.routines` by index in the calling code, to avoid
/// attributing sites to the wrong routine instance when two routines in the
/// same object share a name (e.g. multiple `OnValidate` field triggers in a
/// TableExtension).
///
/// `object_globals` should be the record-typed global variable names from
/// `file.objects[obj_idx]` only.
///
/// Returns an empty `Vec` if either index is out of range or the routine has
/// no body.
pub fn extract_sites_for_routine(
    file: &AlFile,
    src: &str,
    unit: &str,
    object_globals: &HashSet<String>,
    obj_idx: usize,
    routine_idx: usize,
) -> Vec<RawSiteV2> {
    let Some(obj) = file.objects.get(obj_idx) else {
        return Vec::new();
    };
    let Some(routine) = obj.routines.get(routine_idx) else {
        return Vec::new();
    };
    let Some(body) = routine.body else {
        return Vec::new();
    };
    let caller = routine.name.fold_identifier();
    let mut rvars = routine_rvars(routine);
    rvars.extend(object_globals.iter().cloned());
    let ctx = WithCtx::for_routine(src, file, obj_idx, routine);
    let mut out = Vec::new();
    walk_block_v2(file, src, body, unit, &caller, &rvars, ctx, &mut out);
    out.sort_by_key(|a| a.span.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S3.5: a bare record op is a `RecordOp` on the innermost implicit
    /// receiver (L2's rule), and a `Bare` call when that receiver is not a
    /// record or the name is one of the object's own routines.
    #[test]
    fn bare_record_ops_follow_the_implicit_receiver() {
        let src = r#"
table 50100 "T"
{
    procedure A()
    var
        Cust: Record Customer;
        Cu: Codeunit "X";
    begin
        Modify();
        with Cust do
            Insert();
        with Cu do
            Delete();
        Validate(Code);
    end;

    procedure Validate(F: Integer)
    begin
    end;
}

codeunit 50101 "C"
{
    procedure B()
    begin
        Modify();
    end;
}

codeunit 50102 "D"
{
    TableNo = Customer;

    trigger OnRun()
    begin
        Modify();
    end;
}
"#;
        let file = al_syntax::parse(src);
        // Parens-less value reads are candidates, not definite sites.
        let sites: Vec<_> = extract_sites(&file, src, "C.al", &std::collections::HashSet::new())
            .into_iter()
            .filter(|s| !s.parenless)
            .collect();
        let shapes: Vec<(&str, &CalleeShape)> = sites
            .iter()
            .map(|s| (s.caller_routine.as_str(), &s.shape))
            .collect();
        let op = |r: &str, o: &str| CalleeShape::RecordOp {
            receiver_text: r.to_string(),
            op: o.to_string(),
        };
        let bare = |n: &str| CalleeShape::Bare {
            name: n.to_string(),
        };
        assert_eq!(
            shapes,
            vec![
                ("a", &op("Rec", "modify")),
                ("a", &op("Cust", "insert")),
                ("a", &bare("Delete")),
                ("a", &bare("Validate")),
                ("b", &bare("Modify")),
                ("onrun", &op("Rec", "modify")),
            ]
        );
    }

    #[test]
    fn classifies_call_shapes() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Item;
        Json: JsonObject;
    begin
        Foo();
        Helper.Process();
        Rec.SetRange(Status);
        Codeunit.Run(Codeunit::"Other");
        Json.Add('a');
        Commit();
    end;
    procedure Foo() begin end;
}
"#;
        let file = al_syntax::parse(src);
        // Parens-less value reads are candidates, not definite sites.
        let sites: Vec<_> = extract_sites(&file, src, "C.al", &std::collections::HashSet::new())
            .into_iter()
            .filter(|s| !s.parenless)
            .collect();
        let run: Vec<_> = sites.iter().filter(|s| s.caller_routine == "run").collect();
        assert_eq!(run.len(), 6, "Expected 6 call sites in Run procedure");
        assert!(run.iter().any(
            |s| matches!(&s.shape, CalleeShape::Bare { name } if name.eq_ignore_ascii_case("foo"))
        ));
        assert!(
            run.iter()
                .any(|s| matches!(&s.shape, CalleeShape::Member { method, .. } if method.eq_ignore_ascii_case("process")))
        );
        assert!(
            run.iter()
                .any(|s| matches!(&s.shape, CalleeShape::RecordOp { op, .. } if op.eq_ignore_ascii_case("setrange")))
        );
        assert!(
            run.iter()
                .any(|s| matches!(&s.shape, CalleeShape::ObjectRun { .. }))
        );
        assert!(run.iter().any(|s| matches!(&s.shape, CalleeShape::Commit)));
        // Json.Add is a Member call, NOT a RecordOp (Json is not a record).
        assert!(
            run.iter()
                .any(|s| matches!(&s.shape, CalleeShape::Member { receiver_text, method, .. } if receiver_text.eq_ignore_ascii_case("json") && method.eq_ignore_ascii_case("add")))
        );
        assert!(
            !run.iter()
                .any(|s| matches!(&s.shape, CalleeShape::RecordOp { receiver_text, .. } if receiver_text.eq_ignore_ascii_case("json")))
        );
    }

    /// Task 2 invariant (a): for a `Func().M()` call, `CalleeShape::Member`
    /// carries a `receiver: Some(ExprId)` that dereferences (via
    /// `file.ir.expr(id)`) to a STRUCTURED `ExprKind::Call{function, args}`
    /// node whose `args.len()` equals the source argument count — proving the
    /// parsed receiver AST (not just its raw text) reaches the resolver. This
    /// is the primitive Tasks 3-4 consume; Task 2 itself adds no new
    /// resolution behavior (the field is excluded from `CalleeShape`'s
    /// `PartialEq`/`Eq`, see that enum's doc).
    #[test]
    fn member_receiver_threads_structured_call_expr() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    begin
        Func(1, 2, 3).M();
    end;
    procedure Func(A: Integer; B: Integer; C: Integer): Codeunit "C" begin end;
}
"#;
        let file = al_syntax::parse(src);
        let sites = extract_sites(&file, src, "C.al", &std::collections::HashSet::new());
        let run: Vec<_> = sites.iter().filter(|s| s.caller_routine == "run").collect();

        let member_site = run
            .iter()
            .find(|s| matches!(&s.shape, CalleeShape::Member { method, .. } if method.eq_ignore_ascii_case("m")))
            .expect("Func(1, 2, 3).M() must classify as a Member call");

        let CalleeShape::Member { receiver, .. } = &member_site.shape else {
            unreachable!("filtered to Member above");
        };
        let receiver_id = receiver.expect("Member.receiver must be populated by classify_call");

        let receiver_expr = file.ir.expr(receiver_id);
        match &receiver_expr.kind {
            ExprKind::Call { args, .. } => {
                assert_eq!(
                    args.len(),
                    3,
                    "threaded receiver Expr must carry the real source arg count"
                );
            }
            // `ExprKind` derives no `Debug` — assert on the discriminant via
            // `matches!` rather than interpolating the value.
            _ => panic!("expected ExprKind::Call for the Func(...) receiver"),
        }
    }

    /// Receiver-closure plan v2.1 Task 2 addendum ("assignment-LHS never
    /// becomes a call"): `Response.Content := X;` — a plain assignment whose
    /// TARGET is a bare `ExprKind::Member` (no parens, no args ANYWHERE) —
    /// must produce ZERO call sites for that target. This is not a NEW gate
    /// Task 2 had to add: `collect_calls_v2`'s `ExprKind::Member` arm only
    /// ever RECURSES into `object` looking for a nested `Call`, it never
    /// itself emits a `RawSiteV2` for a bare `Member` — so a target with no
    /// `Call` anywhere in it (like this one) is structurally invisible to
    /// call-site extraction, regardless of Task 2's parens-optional
    /// normalization (which lives entirely inside `infer_receiver_type`'s
    /// RECEIVER-TYPING of an ALREADY-EXTRACTED real call, never inside
    /// extraction itself). This test pins that invariant so a future change
    /// to `collect_calls_v2` can't silently start fabricating call edges
    /// from assignment targets.
    #[test]
    fn assignment_lhs_bare_member_produces_no_call_site() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        X: HttpContent;
    begin
        Response.Content := X;
    end;
}
"#;
        let file = al_syntax::parse(src);
        // Parens-less value reads are candidates, not definite sites.
        let sites: Vec<_> = extract_sites(&file, src, "C.al", &std::collections::HashSet::new())
            .into_iter()
            .filter(|s| !s.parenless)
            .collect();
        let run: Vec<_> = sites.iter().filter(|s| s.caller_routine == "run").collect();
        assert!(
            run.is_empty(),
            "a bare-Member assignment target (no Call anywhere) must yield NO call sites, got: {run:?}"
        );
    }

    /// The SAME invariant, but with a genuine call NESTED inside the
    /// assignment target (`GetResponse().Content := X;`) — the nested
    /// `GetResponse()` call IS extracted (it is a real procedure
    /// invocation, unrelated to whether the outer `.Content` is read or
    /// written), while `.Content` itself still produces no separate call
    /// site. Proves the LHS gate is about "no Call node" (this test), not
    /// "no Member node can ever appear in an LHS" (assignment targets with
    /// real calls inside them are unaffected either way).
    #[test]
    fn assignment_lhs_with_nested_call_still_extracts_only_the_real_call() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    begin
        GetResponse().Content := X;
    end;
    procedure GetResponse(): HttpResponseMessage begin end;
}
"#;
        let file = al_syntax::parse(src);
        // Parens-less value reads are candidates, not definite sites.
        let sites: Vec<_> = extract_sites(&file, src, "C.al", &std::collections::HashSet::new())
            .into_iter()
            .filter(|s| !s.parenless)
            .collect();
        let run: Vec<_> = sites.iter().filter(|s| s.caller_routine == "run").collect();
        assert_eq!(
            run.len(),
            1,
            "only the nested GetResponse() call is a real call site, got: {run:?}"
        );
        assert!(run.iter().any(
            |s| matches!(&s.shape, CalleeShape::Bare { name } if name.eq_ignore_ascii_case("getresponse"))
        ));
    }

    // -- routine_has_with_token: comment/string/quoted-identifier aware
    // (Task 4, receiver-closure-and-arg-increments plan) --------------------

    #[test]
    fn with_token_line_comment_excluded() {
        let src = "begin\n  // with something\n  X := 1;\nend;";
        assert!(!routine_has_with_token(src, 0..src.len()));
    }

    #[test]
    fn with_token_block_comment_excluded() {
        let src = "begin\n  /* with something */\n  X := 1;\nend;";
        assert!(!routine_has_with_token(src, 0..src.len()));
    }

    #[test]
    fn with_token_string_literal_excluded() {
        let src = "begin\n  Msg := 'with something';\nend;";
        assert!(!routine_has_with_token(src, 0..src.len()));
    }

    /// AL's `''` doubled-quote escape: the escaped quote must NOT prematurely
    /// close the string, so the "with" that follows it inside the SAME
    /// string literal still counts as excluded.
    #[test]
    fn with_token_string_escaped_quote_still_excludes_embedded_with() {
        let src = "begin\n  Msg := 'it''s with you';\nend;";
        assert!(!routine_has_with_token(src, 0..src.len()));
    }

    #[test]
    fn with_token_quoted_identifier_excluded() {
        // A field/var literally named "with" (quoted identifier), not the
        // keyword.
        let src = "begin\n  X := \"with\";\nend;";
        assert!(!routine_has_with_token(src, 0..src.len()));
    }

    /// The SAME `""` doubled-quote escape, for a quoted identifier.
    #[test]
    fn with_token_quoted_identifier_escaped_quote_still_excludes_embedded_with() {
        let src = "begin\n  X := \"a \"\"with\"\" b\";\nend;";
        assert!(!routine_has_with_token(src, 0..src.len()));
    }

    /// A REAL `with` keyword outside every excluded shape still hits.
    #[test]
    fn with_token_real_keyword_still_hits() {
        let src = "begin\n  with Rec do X := 1;\nend;";
        assert!(routine_has_with_token(src, 0..src.len()));
    }

    /// A comment mentioning "with" AND a real `with` statement later in the
    /// SAME routine — must still hit (the real `with` is the true positive).
    #[test]
    fn with_token_comment_and_real_with_both_present_still_hits() {
        let src = "begin\n  // with something\n  with Rec do X := 1;\nend;";
        assert!(routine_has_with_token(src, 0..src.len()));
    }

    /// Uncertain -> conservative: an unterminated block comment must NOT
    /// silently swallow the rest of the routine — treated as a hit.
    #[test]
    fn with_token_unterminated_block_comment_is_conservative() {
        let src = "begin\n  /* comment never closes";
        assert!(routine_has_with_token(src, 0..src.len()));
    }

    /// Uncertain -> conservative: an unterminated string literal likewise
    /// counts as a hit rather than silently declining to scan the rest.
    #[test]
    fn with_token_unterminated_string_is_conservative() {
        let src = "begin\n  Msg := 'never closes";
        assert!(routine_has_with_token(src, 0..src.len()));
    }

    /// Uncertain -> conservative: an unterminated quoted identifier likewise
    /// counts as a hit.
    #[test]
    fn with_token_unterminated_quoted_identifier_is_conservative() {
        let src = "begin\n  X := \"never closes";
        assert!(routine_has_with_token(src, 0..src.len()));
    }

    /// The real CDO regression shape (`UseContiniaAuthorization`, `Codeunit
    /// 6175322 "CDO Http Management"`, see the CDO harness's ratchet history
    /// for the Task-2-review-fix that first surfaced this): a routine's
    /// LEADING COMMENT mentions "with" as a standalone word, but the routine
    /// has no real `with` block at all — end-to-end through `extract_sites`,
    /// the comment-aware scan must no longer disagree with the AST-depth
    /// signal, restoring `WithState::NoWithProven` for calls inside it.
    #[test]
    fn comment_with_no_real_with_block_is_no_with_proven_end_to_end() {
        let src = r#"
codeunit 50100 "C"
{
    procedure UseSomeAuth()
    var
        Auth: Codeunit "Auth Helper";
    begin
        // Callers running inside a TryFunction must share the same instance
        // with an outside-TryFunction warmup call so this short-circuit
        // takes effect.
        Auth.Authorize(Auth, false);
    end;
}
"#;
        let file = al_syntax::parse(src);
        let sites = extract_sites(&file, src, "C.al", &std::collections::HashSet::new());
        let site = sites
            .iter()
            .find(|s| s.caller_routine == "usesomeauth")
            .expect("must find the Authorize call site");
        assert_eq!(
            site.with_state,
            WithState::NoWithProven,
            "a 'with' inside a COMMENT must no longer disagree with the \
             AST-depth signal (fixed comment-blindness — the exact real CDO \
             shape); got {:?}",
            site.with_state
        );
    }

    /// Control: a REAL `with` block still gates end-to-end (the two-signal
    /// design is unchanged — only the text signal's precision improved).
    #[test]
    fn real_with_block_still_gates_end_to_end() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Item;
    begin
        with Rec do
            Foo();
    end;
    procedure Foo() begin end;
}
"#;
        let file = al_syntax::parse(src);
        let sites = extract_sites(&file, src, "C.al", &std::collections::HashSet::new());
        let site = sites
            .iter()
            .find(|s| matches!(&s.shape, CalleeShape::Bare { name } if name.eq_ignore_ascii_case("foo")))
            .expect("must find the Foo() call site");
        assert_eq!(site.with_state, WithState::InsideWith);
    }
}
