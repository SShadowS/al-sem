//! Diff categories, kinds, and the finding fingerprint. Port of al-sem
//! `src/diff/diff-identity.ts`.

use crate::engine::ids::sha256_hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffCategory {
    Abi,
    Schema,
    Events,
    Capabilities,
    Permissions,
}

impl DiffCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffCategory::Abi => "abi",
            DiffCategory::Schema => "schema",
            DiffCategory::Events => "events",
            DiffCategory::Capabilities => "capabilities",
            DiffCategory::Permissions => "permissions",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    // ABI
    ObjectRemoved,
    ObjectAccessibilityNarrowed,
    ProcedureRemoved,
    ProcedureSignatureChanged,
    ProcedureVarDirectionChanged,
    ProcedureObsoletionRegressed,
    ProcedureObsoletionProgressed,
    ObjectAdded,
    ProcedureAdded,
    // Schema
    TableFieldRemoved,
    TableFieldTypeNarrowed,
    TableFieldTypeWidened,
    TableFieldDataClassificationTightened,
    TableFieldDataClassificationRelaxed,
    EnumValueRemoved,
    EnumValueRenumbered,
    TableFieldAdded,
    EnumValueAdded,
    /// A table key (by its position, `<table>#K<n>`, as the schema facts
    /// identify keys) only the new side has, only the old side has, or whose
    /// fields changed.
    TableKeyAdded,
    TableKeyRemoved,
    TableKeyFieldsChanged,
    // Events
    EventPublisherRemoved,
    EventPublisherSignatureChanged,
    EventPublisherAdded,
    EventSubscriberInducedCapabilityGained,
    EventSubscriberInducedCapabilityLost,
    EventContractChangedWithAffectedSubscribers,
    // Capabilities
    CapabilityGainedWrite,
    CapabilityGainedRead,
    CapabilityGainedCommit,
    CapabilityGainedHttp,
    CapabilityGainedTelemetry,
    CapabilityGainedIsolatedStorage,
    CapabilityGainedFile,
    CapabilityGainedDynamicDispatch,
    CapabilityGainedEventPublish,
    CapabilityLost,
    CapabilityLostUnderCoverage,
    // Permissions
    PermissionRightsExpanded,
    PermissionRightsContracted,
    PermissionTargetAdded,
    PermissionTargetRemoved,
}

impl DiffKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffKind::ObjectRemoved => "object-removed",
            DiffKind::ObjectAccessibilityNarrowed => "object-accessibility-narrowed",
            DiffKind::ProcedureRemoved => "procedure-removed",
            DiffKind::ProcedureSignatureChanged => "procedure-signature-changed",
            DiffKind::ProcedureVarDirectionChanged => "procedure-var-direction-changed",
            DiffKind::ProcedureObsoletionRegressed => "procedure-obsoletion-regressed",
            DiffKind::ProcedureObsoletionProgressed => "procedure-obsoletion-progressed",
            DiffKind::ObjectAdded => "object-added",
            DiffKind::ProcedureAdded => "procedure-added",
            DiffKind::TableFieldRemoved => "table-field-removed",
            DiffKind::TableFieldTypeNarrowed => "table-field-type-narrowed",
            DiffKind::TableFieldTypeWidened => "table-field-type-widened",
            DiffKind::TableFieldDataClassificationTightened => {
                "table-field-data-classification-tightened"
            }
            DiffKind::TableFieldDataClassificationRelaxed => {
                "table-field-data-classification-relaxed"
            }
            DiffKind::EnumValueRemoved => "enum-value-removed",
            DiffKind::EnumValueRenumbered => "enum-value-renumbered",
            DiffKind::TableFieldAdded => "table-field-added",
            DiffKind::EnumValueAdded => "enum-value-added",
            DiffKind::TableKeyAdded => "table-key-added",
            DiffKind::TableKeyRemoved => "table-key-removed",
            DiffKind::TableKeyFieldsChanged => "table-key-fields-changed",
            DiffKind::EventPublisherRemoved => "event-publisher-removed",
            DiffKind::EventPublisherSignatureChanged => "event-publisher-signature-changed",
            DiffKind::EventPublisherAdded => "event-publisher-added",
            DiffKind::EventSubscriberInducedCapabilityGained => {
                "event-subscriber-induced-capability-gained"
            }
            DiffKind::EventSubscriberInducedCapabilityLost => {
                "event-subscriber-induced-capability-lost"
            }
            DiffKind::EventContractChangedWithAffectedSubscribers => {
                "event-contract-changed-with-affected-subscribers"
            }
            DiffKind::CapabilityGainedWrite => "capability-gained-write",
            DiffKind::CapabilityGainedRead => "capability-gained-read",
            DiffKind::CapabilityGainedCommit => "capability-gained-commit",
            DiffKind::CapabilityGainedHttp => "capability-gained-http",
            DiffKind::CapabilityGainedTelemetry => "capability-gained-telemetry",
            DiffKind::CapabilityGainedIsolatedStorage => "capability-gained-isolated-storage",
            DiffKind::CapabilityGainedFile => "capability-gained-file",
            DiffKind::CapabilityGainedDynamicDispatch => "capability-gained-dynamic-dispatch",
            DiffKind::CapabilityGainedEventPublish => "capability-gained-event-publish",
            DiffKind::CapabilityLost => "capability-lost",
            DiffKind::CapabilityLostUnderCoverage => "capability-lost-under-coverage",
            DiffKind::PermissionRightsExpanded => "permission-rights-expanded",
            DiffKind::PermissionRightsContracted => "permission-rights-contracted",
            DiffKind::PermissionTargetAdded => "permission-target-added",
            DiffKind::PermissionTargetRemoved => "permission-target-removed",
        }
    }
}

/// SHA-256(`category|kind|normalizedStableId|secondaryKey`) truncated to 16 hex.
/// Mirrors al-sem `computeDiffFingerprint`. `secondary_key` is the empty string
/// when absent.
pub fn compute_diff_fingerprint(
    category: DiffCategory,
    kind: DiffKind,
    normalized_stable_id: &str,
    secondary_key: Option<&str>,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        category.as_str(),
        kind.as_str(),
        normalized_stable_id,
        secondary_key.unwrap_or("")
    );
    // `sha256_hex` always returns 64 lowercase hex chars, so slicing `[..16]` is
    // always in-bounds and char-boundary-safe (al-sem truncates to 16 hex too).
    let hex = sha256_hex(&payload);
    debug_assert_eq!(hex.len(), 64, "sha256_hex must be 64 hex chars");
    hex[..16].to_string()
}
