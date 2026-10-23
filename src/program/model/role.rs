//! #220 (#160 step 8, spec §4.1 D1): THE role rule. A row (routine or object)
//! is `Primary` when its own `app_guid` is the workspace's app
//! (`Model::primary_app`), compared case-insensitively, else `Dependency`. No
//! primary app means every row is primary. A rule, not a store: nothing keeps a
//! `Vec<Role>` in sync.
//!
//! The `dep:` location filter (`registry.rs`) stays beside it, unchanged; 11b's
//! `projection::role_of` and d1's all-primary anchor map are kept on purpose
//! (spec §3.1).

use super::workspace::{Model, ModelObject, ModelRoutine};

/// Whether a row belongs to the workspace's own app or to a dependency app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Primary,
    Dependency,
}

impl Role {
    /// `"primary"` | `"dependency"`: the value the registry's maps and the
    /// snapshot's `role` fields carry.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Primary => "primary",
            Role::Dependency => "dependency",
        }
    }
}

impl Model {
    /// THE role rule. No primary app => every row is primary.
    #[must_use]
    pub fn role_of_app(&self, app_guid: &str) -> Role {
        match &self.primary_app {
            Some(p) if !p.app_guid.eq_ignore_ascii_case(app_guid) => Role::Dependency,
            _ => Role::Primary,
        }
    }

    /// `role_of_app(&r.app_guid)`.
    #[must_use]
    pub fn routine_role(&self, r: &ModelRoutine) -> Role {
        self.role_of_app(&r.app_guid)
    }

    /// `role_of_app(&o.app_guid)`.
    #[must_use]
    pub fn object_role(&self, o: &ModelObject) -> Role {
        self.role_of_app(&o.app_guid)
    }
}

/// The same rule over a stable id's `<appGuid>:` prefix, for readers that hold
/// snapshot ids and no model rows (selector index, serializer, shards). P1b: the
/// prefix equals the row's `app_guid` on every CDO row and object.
#[must_use]
pub fn role_of_stable_id(primary: Option<&str>, stable_id: &str) -> Role {
    let Some(p) = primary else {
        return Role::Primary;
    };
    let is_primary = stable_id.len() > p.len()
        && stable_id.as_bytes()[p.len()] == b':'
        && stable_id.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    if is_primary {
        Role::Primary
    } else {
        Role::Dependency
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::app_attribution::App;
    use crate::engine::l5::test_support::{empty_resolved, routine};

    const PRIMARY: &str = "aaaa0220-0000-0000-0000-0000000000ab";
    const DEP: &str = "bbbb0220-0000-0000-0000-0000000000cd";

    fn model_with_primary(guid: Option<&str>) -> Model {
        let mut m = empty_resolved();
        m.primary_app = guid.map(|g| App {
            app_guid: g.to_string(),
            publisher: "probe".to_string(),
            name: "Ws".to_string(),
            version: "1.0.0.0".to_string(),
        });
        m
    }

    fn row(app_guid: &str) -> ModelRoutine {
        let mut r = routine("r", "procedure");
        r.app_guid = app_guid.to_string();
        r
    }

    fn object(app_guid: &str) -> ModelObject {
        ModelObject {
            id: format!("{app_guid}/Codeunit/1"),
            app_guid: app_guid.to_string(),
            object_type: "Codeunit".to_string(),
            object_number: 1,
            object_key: "1".to_string(),
            name: "Obj".to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
            declaration_only_routines: Vec::new(),
        }
    }

    /// T1 (spec §5): the rule over rows built by assignment. The primary guid
    /// in upper and lower case reads `Primary`, a dependency guid
    /// `Dependency`, and with no primary app every row is `Primary`. The
    /// stable-id form reads the `<guid>:` prefix, case-insensitively, and a
    /// primary guid NOT followed by `:` is no prefix.
    ///
    /// Breaks: a case-sensitive compare makes the upper-case row `Dependency`;
    /// dropping the `None` arm makes the no-primary row `Dependency`; dropping
    /// the `:` check makes `<primary>x` read `Primary`.
    #[test]
    fn role_rule_hand_stated() {
        let m = model_with_primary(Some(PRIMARY));
        let upper = PRIMARY.to_ascii_uppercase();
        assert_ne!(upper, PRIMARY, "precondition: the guid has letters");

        assert_eq!(m.role_of_app(PRIMARY), Role::Primary);
        assert_eq!(m.role_of_app(&upper), Role::Primary);
        assert_eq!(m.role_of_app(DEP), Role::Dependency);

        assert_eq!(m.routine_role(&row(PRIMARY)), Role::Primary);
        assert_eq!(m.routine_role(&row(&upper)), Role::Primary);
        assert_eq!(m.routine_role(&row(DEP)), Role::Dependency);
        assert_eq!(m.object_role(&object(&upper)), Role::Primary);
        assert_eq!(m.object_role(&object(DEP)), Role::Dependency);

        let none = model_with_primary(None);
        assert_eq!(none.role_of_app(DEP), Role::Primary);
        assert_eq!(none.routine_role(&row(DEP)), Role::Primary);
        assert_eq!(none.object_role(&object(DEP)), Role::Primary);

        assert_eq!(Role::Primary.as_str(), "primary");
        assert_eq!(Role::Dependency.as_str(), "dependency");

        let p = Some(PRIMARY);
        assert_eq!(
            role_of_stable_id(p, &format!("{upper}:Codeunit:1")),
            Role::Primary
        );
        assert_eq!(
            role_of_stable_id(p, &format!("{DEP}:Codeunit:1")),
            Role::Dependency
        );
        assert_eq!(
            role_of_stable_id(p, &format!("{PRIMARY}x")),
            Role::Dependency,
            "a primary guid not followed by `:` is no prefix"
        );
        assert_eq!(
            role_of_stable_id(None, &format!("{DEP}:Codeunit:1")),
            Role::Primary
        );
    }
}
