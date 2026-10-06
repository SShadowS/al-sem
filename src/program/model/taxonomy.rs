//! Strict resolution taxonomy (replaces the stringly-typed `dispatch_kind` /
//! `resolution` TS-port hangover). `enum.as_str()` reproduces the EXACT golden
//! strings at the projection boundary so this refactor is byte-stable.

use super::calls::UnknownReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchKind {
    Direct,
    Interface,
    Builtin,
    Unresolved,
    Dynamic,
    Method,
    ImplicitTrigger,
    PageRun,
    ReportRun,
    CodeunitRun,
}

impl DispatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DispatchKind::Direct => "direct",
            DispatchKind::Interface => "interface",
            DispatchKind::Builtin => "builtin",
            DispatchKind::Unresolved => "unresolved",
            DispatchKind::Dynamic => "dynamic",
            DispatchKind::Method => "method",
            DispatchKind::ImplicitTrigger => "implicit-trigger",
            DispatchKind::PageRun => "page-run",
            DispatchKind::ReportRun => "report-run",
            DispatchKind::CodeunitRun => "codeunit-run",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Resolved,
    Maybe,
    Builtin,
    MemberNotFound,
    Ambiguous,
    Opaque,
    ExternalTarget,
    Unknown(UnknownReason),
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Resolved => "resolved",
            Resolution::Maybe => "maybe",
            Resolution::Builtin => "builtin",
            Resolution::MemberNotFound => "member-not-found",
            Resolution::Ambiguous => "ambiguous",
            Resolution::Opaque => "opaque",
            Resolution::ExternalTarget => "external-target",
            Resolution::Unknown(_) => "unknown",
        }
    }
    pub fn unknown_reason(self) -> Option<UnknownReason> {
        match self {
            Resolution::Unknown(r) => Some(r),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::calls::UnknownReason;
    use super::*;

    #[test]
    fn dispatch_kind_strings_are_golden_stable() {
        assert_eq!(DispatchKind::Direct.as_str(), "direct");
        assert_eq!(DispatchKind::Interface.as_str(), "interface");
        assert_eq!(DispatchKind::Builtin.as_str(), "builtin");
        assert_eq!(DispatchKind::Unresolved.as_str(), "unresolved");
        assert_eq!(DispatchKind::Dynamic.as_str(), "dynamic");
        assert_eq!(DispatchKind::Method.as_str(), "method");
        assert_eq!(DispatchKind::ImplicitTrigger.as_str(), "implicit-trigger");
        assert_eq!(DispatchKind::PageRun.as_str(), "page-run");
        assert_eq!(DispatchKind::ReportRun.as_str(), "report-run");
        assert_eq!(DispatchKind::CodeunitRun.as_str(), "codeunit-run");
    }

    #[test]
    fn resolution_strings_are_golden_stable_and_unknown_folds_reason() {
        assert_eq!(Resolution::Resolved.as_str(), "resolved");
        assert_eq!(Resolution::Maybe.as_str(), "maybe");
        assert_eq!(Resolution::Builtin.as_str(), "builtin");
        assert_eq!(Resolution::MemberNotFound.as_str(), "member-not-found");
        assert_eq!(Resolution::Ambiguous.as_str(), "ambiguous");
        assert_eq!(Resolution::Opaque.as_str(), "opaque");
        assert_eq!(Resolution::ExternalTarget.as_str(), "external-target");
        assert_eq!(
            Resolution::Unknown(UnknownReason::BareUnresolved).as_str(),
            "unknown"
        );
        assert_eq!(
            Resolution::Unknown(UnknownReason::BareUnresolved).unknown_reason(),
            Some(UnknownReason::BareUnresolved)
        );
        assert_eq!(Resolution::Resolved.unknown_reason(), None);
    }
}
