//! The AL TableData permission right a record operation needs: the one
//! mapping every surface that prints or compares required permissions reads
//! (`alsem fingerprint`'s human and query output, the full snapshot's
//! `requiredPermissions` rows).
//!
//! AL spells TableData rights `R` (read), `I` (insert), `M` (modify) and `D`
//! (delete), in that order (`tabledata Customer = RIMD`); `X` (execute) is an
//! object right, not a TableData one. Engine limits B8: the fingerprint once
//! kept its own copy of this table and printed `W` for an insert, a right AL
//! does not have.

/// The TableData rights in AL's own order.
pub const TABLE_RIGHTS: [&str; 4] = ["R", "I", "M", "D"];

/// The TableData right a record operation of kind `op` (`read`, `insert`,
/// `modify`, `delete`) needs; `None` for any other kind.
#[must_use]
pub fn table_op_right(op: &str) -> Option<&'static str> {
    match op {
        "read" => Some("R"),
        "insert" => Some("I"),
        "modify" => Some("M"),
        "delete" => Some("D"),
        _ => None,
    }
}

/// `rights` with `right` added once, kept in AL's order (`RIMD`).
#[must_use]
pub fn with_right(rights: &str, right: &str) -> String {
    TABLE_RIGHTS
        .iter()
        .copied()
        .filter(|r| *r == right || rights.contains(*r))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_kind_maps_to_its_al_right() {
        assert_eq!(table_op_right("read"), Some("R"));
        assert_eq!(table_op_right("insert"), Some("I"));
        assert_eq!(table_op_right("modify"), Some("M"));
        assert_eq!(table_op_right("delete"), Some("D"));
        assert_eq!(table_op_right("execute"), None);
    }

    #[test]
    fn rights_keep_al_order() {
        let mut r = String::new();
        for right in ["D", "M", "I", "R", "M"] {
            r = with_right(&r, right);
        }
        assert_eq!(r, "RIMD");
        assert_eq!(with_right("M", "R"), "RM");
    }
}
