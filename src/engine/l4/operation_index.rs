//! The operation index (MCP output wave 2, W2-0a; design
//! `docs/superpowers/specs/2026-11-04-mcp-output-wave2-design.md` §3.5): every
//! operation id and call-site id of the model, mapped to EVERY routine row that
//! claims it.
//!
//! Ids have the form `{app hash}/{routine id}/op{N}` and `…/cs{N}`
//! (`program::body::ir_walk`): the routine part is the model's INTERNAL routine
//! id, so only collided rows (two declarations sharing one internal id, for
//! example the two arms of an `#if`) share an id. Such an id has several claims,
//! and the index keeps them all, in model row order. It never picks one silently:
//!
//! - [`OperationIndex::lookup`] says `Found`, `Ambiguous` or `Missing`, so new
//!   presentation sees the ambiguity;
//! - two existing joins keep the one claim they always took, through their own
//!   named accessor: `query effects`' `origin` takes the LAST claiming row
//!   ([`OperationIndex::last_record_operation`], #160 step 11b §4.6, class P;
//!   ConTinAI reads `origin` directly), and explain takes the FIRST
//!   ([`OperationIndex::first_record_operation_row`]).
//!
//! An operation id names a record operation (`Insert`, `Modify`, …, which is
//! also listed as an operation site), a `Commit` / `Error` operation site, or a
//! call site's own operation (`cs.operation_id`, numbered after the routine's
//! operations, so the spaces never overlap). A call-site id (`…/cs{N}`) is
//! indexed apart.

use crate::program::body::features::{PAnchor, PCallSite, POperationSite};
use crate::program::model::workspace::{Model, ModelRecordOperation, ModelRoutine};
use smallvec::SmallVec;
use std::collections::HashMap;

/// One routine row's claim on an operation id: the row (an index into
/// `model.workspace.routines`) and where in that row the id sits. A record
/// operation is also an operation site, so one claim can hold both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpClaim {
    pub row: u32,
    /// Index into the row's `record_operations`, then its
    /// `field_ref_operations` ([`ModelRoutine::effect_operation`]).
    pub record_operation: Option<u32>,
    /// Index into the row's `operation_sites`.
    pub operation_site: Option<u32>,
    /// Index into the row's `call_sites` (the id is that call site's
    /// `operation_id`).
    pub call_site: Option<u32>,
}

/// What [`OperationIndex::lookup`] found for one id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerLookup<'i> {
    /// Exactly one row claims the id.
    Found(OpClaim),
    /// Several rows claim it (collided declarations), in model row order.
    Ambiguous(&'i [OpClaim]),
    /// No loaded body holds it (a symbol-only or unfetched dependency, or an
    /// id from another model).
    Missing,
}

type Claims = SmallVec<[OpClaim; 1]>;

/// What an index holds. The two legacy joins read record operations only, so
/// they build no more than they did before the index existed (declared-needs
/// rule 4: a fact nobody reads is not built); readers of every id kind
/// (owners of commit sites, call-site anchors) build [`IndexScope::All`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexScope {
    /// Record operations only: enough for [`OperationIndex::last_record_operation`]
    /// and [`OperationIndex::first_record_operation_row`].
    RecordOperations,
    /// Every operation id (record operations, operation sites, call-site
    /// operations) and every call-site id.
    All,
}

/// The index over one model (module doc).
pub struct OperationIndex<'m> {
    model: &'m Model,
    scope: IndexScope,
    ops: HashMap<&'m str, Claims>,
    /// Call-site id → the rows declaring it, each with the call site's index.
    call_sites: HashMap<&'m str, SmallVec<[(u32, u32); 1]>>,
}

/// The claim of `row` on `id`, appended or merged into the row's last claim
/// (a row's own ids are visited together, so its claim is always the last).
fn claim_mut(claims: &mut Claims, row: u32) -> &mut OpClaim {
    if claims.last().is_none_or(|c| c.row != row) {
        claims.push(OpClaim {
            row,
            record_operation: None,
            operation_site: None,
            call_site: None,
        });
    }
    claims.last_mut().expect("just pushed")
}

impl<'m> OperationIndex<'m> {
    /// Index the ids of `model` that `scope` names.
    #[must_use]
    pub fn build(model: &'m Model, scope: IndexScope) -> Self {
        let mut ops: HashMap<&'m str, Claims> = HashMap::new();
        let mut call_sites: HashMap<&'m str, SmallVec<[(u32, u32); 1]>> = HashMap::new();
        for (ri, r) in model.workspace.routines.iter().enumerate() {
            let row = u32::try_from(ri).expect("a routine row fits u32");
            // The record operations, then the `FieldRef` operations (engine
            // limits B5): every operation an effect row can name.
            for (i, op) in r.effect_operations().enumerate() {
                claim_mut(ops.entry(op.id.as_str()).or_default(), row).record_operation =
                    Some(u32::try_from(i).expect("fits u32"));
            }
            if scope == IndexScope::RecordOperations {
                continue;
            }
            for (i, site) in r.operation_sites.iter().enumerate() {
                claim_mut(ops.entry(site.id.as_str()).or_default(), row).operation_site =
                    Some(u32::try_from(i).expect("fits u32"));
            }
            for (i, cs) in r.call_sites.iter().enumerate() {
                let i = u32::try_from(i).expect("fits u32");
                if !cs.operation_id.is_empty() {
                    claim_mut(ops.entry(cs.operation_id.as_str()).or_default(), row).call_site =
                        Some(i);
                }
                call_sites.entry(cs.id.as_str()).or_default().push((row, i));
            }
        }
        OperationIndex {
            model,
            scope,
            ops,
            call_sites,
        }
    }

    /// A reader of every id kind on a record-operations index would read
    /// "missing" for an id that was never indexed: fail loudly instead
    /// (declared-needs rule 5).
    fn require_all(&self, what: &str) {
        assert!(
            self.scope == IndexScope::All,
            "operation index built with IndexScope::RecordOperations: {what} needs IndexScope::All"
        );
    }

    /// Every claim on operation id `op`, in model row order (empty: none).
    /// Needs [`IndexScope::All`].
    #[must_use]
    pub fn claims(&self, op: &str) -> &[OpClaim] {
        self.require_all("claims");
        self.record_claims(op)
    }

    /// The claims as stored, under either scope.
    fn record_claims(&self, op: &str) -> &[OpClaim] {
        self.ops.get(op).map_or(&[], SmallVec::as_slice)
    }

    /// Who owns operation `op`, ambiguity exposed (module doc). Needs
    /// [`IndexScope::All`].
    #[must_use]
    pub fn lookup(&self, op: &str) -> OwnerLookup<'_> {
        match self.claims(op) {
            [] => OwnerLookup::Missing,
            [one] => OwnerLookup::Found(*one),
            many => OwnerLookup::Ambiguous(many),
        }
    }

    /// The rows declaring call site `cs`, each with the call site's index in
    /// the row, in model row order.
    #[must_use]
    pub fn call_site_claims(&self, cs: &str) -> &[(u32, u32)] {
        self.require_all("call_site_claims");
        self.call_sites.get(cs).map_or(&[], SmallVec::as_slice)
    }

    /// The routine row of a claim.
    #[must_use]
    pub fn routine(&self, c: &OpClaim) -> &'m ModelRoutine {
        &self.model.workspace.routines[c.row as usize]
    }

    /// The record operation of a claim, if the id names one.
    #[must_use]
    pub fn record_operation(&self, c: &OpClaim) -> Option<&'m ModelRecordOperation> {
        let r = self.routine(c);
        c.record_operation
            .and_then(|i| r.effect_operation(i as usize))
    }

    /// The operation site of a claim, if the id names one.
    #[must_use]
    pub fn operation_site(&self, c: &OpClaim) -> Option<&'m POperationSite> {
        let r = self.routine(c);
        c.operation_site.map(|i| &r.operation_sites[i as usize])
    }

    /// The call site of a claim, if the id is a call site's operation.
    #[must_use]
    pub fn call_site(&self, c: &OpClaim) -> Option<&'m PCallSite> {
        let r = self.routine(c);
        c.call_site.map(|i| &r.call_sites[i as usize])
    }

    /// The ORIGINAL operation kind of a claim (`Insert`, `ModifyAll`,
    /// `Validate`, `LockTable`, a read…; `commit` / `error-call` for those
    /// sites), never the digest's three write types.
    #[must_use]
    pub fn original_kind(&self, c: &OpClaim) -> Option<&'m str> {
        self.record_operation(c)
            .map(|o| o.op.as_str())
            .or_else(|| self.operation_site(c).map(|s| s.kind.as_str()))
    }

    /// Where a claim's operation sits in source.
    #[must_use]
    pub fn anchor(&self, c: &OpClaim) -> Option<&'m PAnchor> {
        self.record_operation(c)
            .map(|o| &o.source_anchor)
            .or_else(|| self.operation_site(c).map(|s| &s.source_anchor))
            .or_else(|| self.call_site(c).map(|s| &s.source_anchor))
    }

    /// `query effects`' origin join: the LAST row whose record operations hold
    /// `op` (#160 step 11b §4.6, class P), with that operation.
    #[must_use]
    pub fn last_record_operation(
        &self,
        op: &str,
    ) -> Option<(&'m ModelRoutine, &'m ModelRecordOperation)> {
        self.record_claims(op)
            .iter()
            .rev()
            .find_map(|c| Some((self.routine(c), self.record_operation(c)?)))
    }

    /// Explain's join: the FIRST row whose record operations hold `op`.
    #[must_use]
    pub fn first_record_operation_row(&self, op: &str) -> Option<usize> {
        self.record_claims(op)
            .iter()
            .find(|c| c.record_operation.is_some())
            .map(|c| c.row as usize)
    }

    /// [`Self::first_record_operation_row`] for every record operation id, as
    /// an owned map (explain keeps its indexes per revision, past the model
    /// borrow).
    #[must_use]
    pub fn first_record_operation_rows(&self) -> HashMap<String, usize> {
        self.ops
            .keys()
            .filter_map(|op| Some(((*op).to_string(), self.first_record_operation_row(op)?)))
            .collect()
    }
}
