//! The AL compiler's method id: the number BC's transpiled C# names a routine by
//! (`[MethodId(n)]` on the method, `InvokeAsync(n, …)` at a call), and the `Id` of
//! a method in `SymbolReference.json`.
//!
//! Synthesized source for a runtime package without AL (spec
//! `2026-10-28-transpiled-runtime-bodies-design.md` §8.3) can name a call into
//! another app only by this id: the C# carries no method name, and the target app
//! may be source-only (no symbols to look the id up in). The resolver binds such a
//! call by computing the id of each candidate routine from its declared
//! signature ([`method_id`]).
//!
//! The rule (runtime ≥ Spring 2021, alc 18.0 `MethodSymbol.CalculateMethodId`,
//! reimplemented from its behaviour, checked against alc-compiled probes and
//! against every computable method of a real package's symbols):
//! - FNV-1a over the UTF-16LE bytes of the upper-cased name;
//! - [`combine`] with the return type's kind (0 for none);
//! - per parameter, `combine` its index, its `var` flag (1/0) and its kind, the
//!   `_ReturnValue` bit cleared for the kinds [`MASKED_KINDS`] lists, an array
//!   being `Array`;
//! - when the routine can be overloaded (not an event publisher or subscriber)
//!   and a parameter's kind is in [`SUBTYPE_KINDS`], each parameter also
//!   combines its subtype hash: an object type's object id, an interface's
//!   FNV-1a of its declared (not upper-cased) name, 1 for any other type.
//!   A generic (`List`, `Dictionary`) or `DotNet` parameter's subtype hash is not
//!   reproducible here: such a signature has no id ([`None`]), so a call to it
//!   stays unresolved.

use al_syntax::ir::ObjectKind;

/// alc's `Hash.Combine`: `((a << 5) + a + (a >> 27)) ^ b` on `i32`, the shift
/// arithmetic.
pub(crate) fn combine(a: i32, b: i32) -> i32 {
    (a.wrapping_shl(5).wrapping_add(a).wrapping_add(a >> 27)) ^ b
}

/// FNV-1a over the UTF-16LE bytes of `s`.
pub(crate) fn fnv_utf16(s: &str) -> i32 {
    let mut h: i32 = -2128831035;
    for unit in s.encode_utf16() {
        for b in unit.to_le_bytes() {
            h = (h ^ i32::from(b)).wrapping_mul(16777619);
        }
    }
    h
}

/// The `_ReturnValue` bit of a kind.
const RETURN_VALUE_BIT: i32 = 524288;

/// The kind of an array parameter.
const ARRAY: i32 = 1;

/// `NavTypeKind` values of the AL type keywords a signature can name. Matched
/// case-insensitively against a type's first word.
const KINDS: &[(&str, i32)] = &[
    ("Boolean", 12451842),
    ("Byte", 3014659),
    ("Char", 3014660),
    ("Integer", 12451845),
    ("BigInteger", 12451846),
    ("Decimal", 12451847),
    ("Option", 12451848),
    ("Text", 16646153),
    ("Code", 16646154),
    ("DateTime", 12451853),
    ("Time", 12451854),
    ("Date", 12451855),
    ("DateFormula", 12451856),
    ("Duration", 12451857),
    ("Guid", 12451858),
    ("Enum", 12451896),
    ("Action", 983060),
    ("TableConnectionType", 983061),
    ("SecurityFilter", 983062),
    ("TransactionType", 983063),
    ("DefaultLayout", 983064),
    ("TextEncoding", 983065),
    ("ExecutionMode", 983066),
    ("ObjectType", 983067),
    ("ClientType", 983069),
    ("ReportFormat", 983070),
    ("FieldType", 983072),
    ("FieldClass", 983073),
    ("NotificationScope", 983074),
    ("Notification", 917539),
    ("Verbosity", 983076),
    ("DataClassification", 983077),
    ("TelemetryScope", 983078),
    ("CommitBehavior", 65575),
    ("RecordId", 12451880),
    ("RecordRef", 917545),
    ("FieldRef", 917546),
    ("KeyRef", 917547),
    ("TableFilter", 11534380),
    ("InStream", 917549),
    ("OutStream", 917550),
    ("BigText", 3014703),
    ("Blob", 3145776),
    ("PageStyle", 983089),
    ("Variant", 917554),
    ("DotNet", 393268),
    ("Media", 11534389),
    ("MediaSet", 11534390),
    ("TestAction", 70),
    ("TestField", 71),
    ("TestRequestPage", 262216),
    ("TestPage", 393289),
    ("TestFilterField", 74),
    ("TestPart", 75),
    ("TestFilter", 76),
    ("File", 393296),
    ("FilterPageBuilder", 917585),
    ("Dialog", 131154),
    ("SessionSettings", 917587),
    ("JsonToken", 917594),
    ("JsonObject", 917595),
    ("JsonArray", 917596),
    ("JsonValue", 917597),
    ("HttpClient", 917598),
    ("HttpRequestMessage", 917599),
    ("HttpResponseMessage", 917600),
    ("HttpHeaders", 917601),
    ("HttpContent", 917602),
    ("TextBuilder", 917603),
    ("Record", 917604),
    ("Codeunit", 917605),
    ("Page", 917606),
    ("Report", 917607),
    ("XmlPort", 917608),
    ("Query", 917609),
    ("ControlAddIn", 393325),
    ("XmlAttribute", 917624),
    ("XmlAttributeCollection", 917625),
    ("XmlComment", 917626),
    ("XmlCData", 917627),
    ("XmlDeclaration", 917628),
    ("XmlDocument", 917629),
    ("XmlDocumentType", 917630),
    ("XmlElement", 917631),
    ("XmlNamespaceManager", 917632),
    ("XmlNameTable", 917633),
    ("XmlNode", 917634),
    ("XmlNodeList", 917635),
    ("XmlProcessingInstruction", 917636),
    ("XmlText", 917637),
    ("XmlReadOptions", 917638),
    ("XmlWriteOptions", 917639),
    ("TestPermissions", 983238),
    ("TransactionModel", 983239),
    ("List", 917710),
    ("Dictionary", 917711),
    ("ExecutionContext", 983248),
    ("Version", 917713),
    ("ModuleInfo", 917714),
    ("ModuleDependencyInfo", 917715),
    ("WebServiceActionContext", 393428),
    ("WebServiceActionResultCode", 393429),
    ("DataScope", 983254),
    ("ErrorType", 983255),
    ("ErrorInfo", 917720),
    ("PageBackgroundTaskErrorLevel", 983257),
    ("Interface", 917722),
    ("AuditCategory", 983265),
    ("SecurityOperationResult", 983266),
    ("ErrorBehavior", 65763),
    ("PermissionObjectType", 65764),
    ("ReportLayoutType", 983269),
    ("InherentPermissionsScope", 65766),
    ("DataTransfer", 917735),
    ("IsolationLevel", 983272),
    ("SecretText", 917737),
    ("PromptMode", 983274),
    ("FileUpload", 917739),
    ("Cookie", 917740),
    ("HttpRequestType", 983277),
    ("TestHttpRequestMessage", 917742),
    ("TestHttpResponseMessage", 917743),
    ("DataSourceContext", 917744),
    ("TestHandlerContext", 917745),
];

/// The kinds whose `_ReturnValue` bit a parameter's kind clears
/// (`ReturnValuesAddedForRuntimeVersion7`).
const MASKED_KINDS: &[&str] = &[
    "TestRequestPage",
    "TestPage",
    "File",
    "Notification",
    "RecordRef",
    "FieldRef",
    "KeyRef",
    "InStream",
    "OutStream",
    "Variant",
    "FilterPageBuilder",
    "SessionSettings",
    "HttpClient",
    "HttpRequestMessage",
    "HttpResponseMessage",
    "HttpContent",
    "Record",
    "Codeunit",
    "Page",
    "Report",
    "XmlPort",
    "Query",
    "List",
    "Dictionary",
    "Interface",
    "NotificationScope",
    "DateFormula",
    "RecordId",
];

/// The kinds that make an overloadable routine combine every parameter's
/// subtype hash (`IsParameterWithSubtype`).
const SUBTYPE_KINDS: &[&str] = &[
    "TestRequestPage",
    "DotNet",
    "TestPage",
    "Record",
    "Codeunit",
    "Page",
    "XmlPort",
    "Query",
    "List",
    "Dictionary",
    "Interface",
    "Enum",
];

/// A declared type, as far as the method id reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeShape {
    /// The canonical keyword (`KINDS`' spelling), `"Array"` for an array.
    keyword: &'static str,
    kind: i32,
    /// The text after the keyword (an object type's reference, an
    /// interface's name), trimmed, `temporary` removed. Empty when none.
    pub(crate) subtype_text: String,
}

impl TypeShape {
    /// The object kind whose id is this type's subtype hash, if it is one.
    pub(crate) fn subtype_object_kind(&self) -> Option<ObjectKind> {
        Some(match self.keyword {
            "Record" => ObjectKind::Table,
            "Codeunit" => ObjectKind::Codeunit,
            "Page" | "TestPage" => ObjectKind::Page,
            "XmlPort" => ObjectKind::XmlPort,
            "Query" => ObjectKind::Query,
            "Enum" => ObjectKind::Enum,
            "TestRequestPage" => ObjectKind::Report,
            _ => return None,
        })
    }

    pub(crate) fn is_interface(&self) -> bool {
        self.keyword == "Interface"
    }

    fn triggers_subtypes(&self) -> bool {
        SUBTYPE_KINDS.contains(&self.keyword)
    }

    /// A subtype hash this module cannot reproduce.
    fn subtype_unreproducible(&self) -> bool {
        matches!(self.keyword, "List" | "Dictionary" | "DotNet")
    }
}

/// The shape of a declared type text (`Record Customer temporary`,
/// `Text[100]`, `array[10] of Text`, `Enum "Price Source Type"`), or `None` for
/// a keyword this module does not know.
pub(crate) fn parse_type(text: &str) -> Option<TypeShape> {
    let text = text.trim();
    if text.len() >= 5 && text[..5].eq_ignore_ascii_case("array") {
        return Some(TypeShape {
            keyword: "Array",
            kind: ARRAY,
            subtype_text: String::new(),
        });
    }
    let end = text
        .find(|c: char| c.is_whitespace() || c == '[')
        .unwrap_or(text.len());
    let (word, rest) = text.split_at(end);
    let &(keyword, kind) = KINDS.iter().find(|(k, _)| k.eq_ignore_ascii_case(word))?;
    let rest = rest.trim();
    // `Text[100]`: the length is not part of the id.
    let rest = if rest.starts_with('[') {
        rest.find(']').map_or("", |i| rest[i + 1..].trim())
    } else {
        rest
    };
    let subtype_text = rest
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("temporary"))
        .collect::<Vec<_>>()
        .join(" ");
    Some(TypeShape {
        keyword,
        kind,
        subtype_text,
    })
}

/// One parameter of a signature: its `var` flag and type.
pub(crate) struct SigParam {
    pub(crate) is_var: bool,
    pub(crate) ty: TypeShape,
}

/// The method id of a routine named `name` returning `ret` (`None`: no return
/// value) with `params`. `overloadable`: not an event publisher or subscriber.
/// `subtype_hash(i, ty)` gives parameter `i`'s object id or interface's declared
/// name (the caller resolves the reference); its `None` means the reference did
/// not resolve, and then the id is `None`, as it is for a generic or `DotNet`
/// parameter whose subtype hash counts.
pub(crate) fn method_id(
    name: &str,
    ret: Option<&TypeShape>,
    params: &[SigParam],
    overloadable: bool,
    subtype_hash: impl Fn(usize, &TypeShape) -> Option<SubtypeHash>,
) -> Option<i32> {
    let mut h = combine(fnv_utf16(&name.to_uppercase()), ret.map_or(0, |t| t.kind));
    let with_subtypes = overloadable && params.iter().any(|p| p.ty.triggers_subtypes());
    for (i, p) in params.iter().enumerate() {
        let kind = if MASKED_KINDS.contains(&p.ty.keyword) {
            p.ty.kind & !RETURN_VALUE_BIT
        } else {
            p.ty.kind
        };
        h = combine(combine(combine(h, i as i32), i32::from(p.is_var)), kind);
        if with_subtypes {
            let sub = if p.ty.subtype_unreproducible() {
                return None;
            } else if p.ty.subtype_object_kind().is_some() || p.ty.is_interface() {
                match subtype_hash(i, &p.ty)? {
                    SubtypeHash::ObjectId(id) => id,
                    SubtypeHash::InterfaceName(n) => fnv_utf16(&n),
                }
            } else {
                1
            };
            h = combine(h, sub);
        }
    }
    Some(h)
}

/// What a parameter's subtype resolved to.
pub(crate) enum SubtypeHash {
    ObjectId(i32),
    /// The interface's declared name, as the object declares it.
    InterfaceName(String),
}

/// The method id a synthesized call names: `__alsem_mid_<n>`.
pub(crate) fn synthesized_call_id(name: &str) -> Option<i32> {
    let n = name.trim_matches('"');
    let digits = n
        .get(..12)
        .filter(|p| p.eq_ignore_ascii_case("__alsem_mid_"))
        .map(|_| &n[12..])?;
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(name: &str, ret: Option<&str>, params: &[(bool, &str)]) -> Option<i32> {
        let params: Vec<SigParam> = params
            .iter()
            .map(|&(is_var, t)| SigParam {
                is_var,
                ty: parse_type(t).expect("known type"),
            })
            .collect();
        let ret = ret.map(|t| parse_type(t).expect("known type"));
        method_id(name, ret.as_ref(), &params, true, |_, t| {
            Some(SubtypeHash::ObjectId(match t.subtype_text.as_str() {
                "\"Hash T\"" => 50304,
                "\"Hash Probe\"" => 50300,
                other => panic!("unexpected subtype {other}"),
            }))
        })
    }

    /// Ids alc/altool 18.0.43.1464 assigned to a probe app's methods
    /// (2026-10-28): the name is case-insensitive, a length is ignored, `var`
    /// and the return type count, and an object parameter adds its object id.
    /// A probe signature: name, return type, `(var, type)` per parameter, and
    /// the id alc gave it.
    type Probe<'a> = (&'a str, Option<&'a str>, &'a [(bool, &'a str)], i32);

    #[test]
    fn matches_alc_compiled_method_ids() {
        let cases: &[Probe] = &[
            ("A", None, &[], -981080143),
            ("a", None, &[], -981080143),
            ("B", None, &[], 2021342778),
            ("AB", None, &[], 1554881421),
            ("A", None, &[(false, "Integer")], 315586011),
            ("A", None, &[(true, "Integer")], 315585976),
            ("A", None, &[(false, "Text")], 311391703),
            ("A", None, &[(false, "Text[10]")], 311391703),
            (
                "A",
                None,
                &[(false, "Integer"), (false, "Text")],
                -1779338784,
            ),
            (
                "A",
                None,
                &[(false, "Code[20]"), (false, "Decimal")],
                2088084390,
            ),
            ("C", Some("Integer"), &[(true, "Integer")], 1626744269),
            ("c2", Some("Integer"), &[(false, "Integer")], 2081708688),
            ("D", None, &[(false, "Integer")], 413752848),
            ("E", None, &[], 742312314),
            ("F", None, &[(false, "Record \"Hash T\"")], -1627626749),
            ("G", None, &[(false, "Codeunit \"Hash Probe\"")], -594227850),
        ];
        for &(name, ret, params, want) in cases {
            assert_eq!(sig(name, ret, params), Some(want), "{name}{params:?}");
        }
    }

    #[test]
    fn generic_parameters_with_subtypes_have_no_id() {
        let params = vec![
            SigParam {
                is_var: false,
                ty: parse_type("Record \"Hash T\"").unwrap(),
            },
            SigParam {
                is_var: false,
                ty: parse_type("List of [Text]").unwrap(),
            },
        ];
        let id = method_id("F", None, &params, true, |_, _| {
            Some(SubtypeHash::ObjectId(50304))
        });
        assert_eq!(id, None);
    }

    #[test]
    fn parses_type_texts() {
        let t = parse_type("Record Customer temporary").unwrap();
        assert_eq!((t.keyword, t.subtype_text.as_str()), ("Record", "Customer"));
        assert_eq!(parse_type("array[10] of Text").unwrap().kind, ARRAY);
        assert_eq!(parse_type("code[20]").unwrap().keyword, "Code");
        assert_eq!(parse_type("NoSuchType"), None);
        assert_eq!(synthesized_call_id("\"__alsem_mid_-42\""), Some(-42));
        assert_eq!(synthesized_call_id("Foo"), None);
    }
}
