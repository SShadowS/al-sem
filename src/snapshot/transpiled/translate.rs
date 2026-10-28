//! C# method → AL routine body: the statements of an AL routine, rebuilt from
//! the C# BC transpiled it into ([`super::cs`]).
//!
//! The rule is to give back the AL call each AL-runtime call came from, on a
//! variable declared as the matching AL type, so al-sem's own extractors read
//! the synthesized body exactly as they read real AL: `rec.Target.ALModifyAsync`
//! is `"rec".Modify()`, `ALDatabase.ALCommit()` is `Commit()`, a `NavHttpClient`'s
//! `ALPostAsync` is `"client".Post(…)`. What is kept:
//! - control flow (`if`, loops, `case`, `exit`), every condition an opaque
//!   Boolean (`"alsem branch"`): which branch runs is not read, the structure is;
//! - record operations that read or write the database, raise an error or run
//!   trigger code; `TrapError` (the AL `if Rec.Get() then`) consumes the
//!   Boolean, `ThrowError` is a statement;
//! - calls into AL code (codeunit, table and page methods, the object's own
//!   procedures), named by method id (`"__alsem_mid_<n>"`, bound by the
//!   resolver), record arguments passed through so temporariness flows;
//! - every other AL-runtime call on an AL-typed variable, and the known
//!   effectful statics.
//!
//! A call the translator cannot place becomes `"__alsem_untranslated"('<shape>')`:
//! an undeclared procedure, so an unresolved call naming its shape. Nothing is
//! dropped silently but the calls [`is_pure_static`] and [`PURE_INSTANCE`] name.
//! Every emitted statement carries `// cs <line>`, the C# line it came from.

use super::cs::{
    Call, Class, Member, Method, Param, Stmt, Tok, Token, calls, first_line, matching, split_commas,
};
use std::collections::HashMap;

/// An AL variable a C# variable stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VarType {
    /// `Record <id>`, its temporariness when the C# says (a local's
    /// initializer; a parameter's is the caller's).
    Record {
        table: Option<i64>,
        temporary: Option<bool>,
    },
    /// `Codeunit <id>`, `Page <id>`, `Report <id>`, `Query <id>`, `XmlPort <id>`.
    Object { kind: &'static str, id: Option<i64> },
    /// Any other AL type, by its AL name (`HttpClient`, `Text`, …).
    Plain(String),
    /// An interface handle: dispatch the C# does not resolve.
    Interface,
    /// A C# type with no AL counterpart this module knows.
    Unknown,
}

impl VarType {
    /// The AL type text to declare, `None` when it cannot be declared.
    pub(crate) fn al_type(&self) -> Option<String> {
        match self {
            VarType::Record {
                table: Some(t),
                temporary,
            } => Some(if *temporary == Some(true) {
                format!("Record {t} temporary")
            } else {
                format!("Record {t}")
            }),
            VarType::Object { kind, id: Some(id) } => Some(format!("{kind} {id}")),
            VarType::Plain(p) => Some(p.clone()),
            _ => None,
        }
    }
}

/// The AL type of a C# runtime type name (`NavHttpClient` → `HttpClient`).
fn plain_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "bool" | "Boolean" => "Boolean",
        "int" | "Int32" => "Integer",
        "long" | "Int64" | "NavBigInteger" => "BigInteger",
        "decimal" | "Decimal" | "NavDecimal" => "Decimal",
        "string" | "String" | "NavText" => "Text",
        "NavCode" => "Code[2048]",
        "Guid" | "NavGuid" => "Guid",
        "NavDateTime" => "DateTime",
        "NavDate" => "Date",
        "NavTime" => "Time",
        "NavDuration" => "Duration",
        "NavDateFormula" => "DateFormula",
        "NavOption" => "Integer",
        "NavBigText" => "BigText",
        "NavTextBuilder" => "TextBuilder",
        "NavSecretText" => "SecretText",
        "NavInStream" => "InStream",
        "NavOutStream" => "OutStream",
        "NavRecordRef" => "RecordRef",
        "NavFieldRef" => "FieldRef",
        "NavKeyRef" => "KeyRef",
        "NavRecordId" => "RecordId",
        "NavVariant" => "Variant",
        "NavJsonObject" => "JsonObject",
        "NavJsonArray" => "JsonArray",
        "NavJsonToken" => "JsonToken",
        "NavJsonValue" => "JsonValue",
        "NavHttpClient" => "HttpClient",
        "NavHttpRequestMessage" => "HttpRequestMessage",
        "NavHttpResponseMessage" => "HttpResponseMessage",
        "NavHttpContent" => "HttpContent",
        "NavHttpHeaders" => "HttpHeaders",
        "NavXmlDocument" => "XmlDocument",
        "NavXmlElement" => "XmlElement",
        "NavXmlNode" => "XmlNode",
        "NavXmlNodeList" => "XmlNodeList",
        "NavXmlAttribute" => "XmlAttribute",
        "NavXmlAttributeCollection" => "XmlAttributeCollection",
        "NavXmlText" => "XmlText",
        "NavXmlCData" => "XmlCData",
        "NavXmlComment" => "XmlComment",
        "NavXmlDeclaration" => "XmlDeclaration",
        "NavXmlNamespaceManager" => "XmlNamespaceManager",
        "NavXmlNameTable" => "XmlNameTable",
        "NavXmlReadOptions" => "XmlReadOptions",
        "NavXmlWriteOptions" => "XmlWriteOptions",
        "NavXmlProcessingInstruction" => "XmlProcessingInstruction",
        "NavXmlDocumentType" => "XmlDocumentType",
        "NavNotification" => "Notification",
        "NavALErrorInfo" => "ErrorInfo",
        "NavModuleInfo" => "ModuleInfo",
        "NavModuleDependencyInfo" => "ModuleDependencyInfo",
        "NavSessionSettings" => "SessionSettings",
        "NavFilterPageBuilder" => "FilterPageBuilder",
        "NavMedia" => "Media",
        "NavMediaSet" => "MediaSet",
        "NavFile" => "File",
        "NavDialog" | "NavDialogHandle" => "Dialog",
        "NavBLOB" | "NavBlob" => "Codeunit \"Temp Blob\"",
        "NavVersion" => "Version",
        "NavDataTransfer" => "DataTransfer",
        "NavFileUpload" => "FileUpload",
        "NavCookie" => "Cookie",
        _ => return None,
    })
}

/// A C# type's runtime class name: the last identifier before any generic
/// arguments, `ByRef<T>`/`NavArray<T>` unwrapped to `T`'s, namespaces dropped.
fn type_name(ty: &[Token]) -> (String, Option<String>) {
    let idents: Vec<&str> = ty.iter().filter_map(Token::ident).collect();
    // `ByRef<NavText>` → `NavText`; `NavList<NavText>` → (NavList, NavText).
    let lt = ty.iter().position(|t| t.is_punct("<"));
    let head = match lt {
        Some(p) => ty[..p].iter().rev().find_map(Token::ident).unwrap_or(""),
        None => idents.last().copied().unwrap_or(""),
    };
    let inner = lt.and_then(|p| ty[p + 1..].iter().find_map(Token::ident));
    if matches!(head, "ByRef" | "NavArray" | "Nullable")
        && let Some(p) = lt
    {
        // The wrapped type, whole: `ByRef<System.Guid>` is `Guid`.
        let close = ty.iter().rposition(|t| t.is_punct(">")).unwrap_or(ty.len());
        return type_name(&ty[p + 1..close]);
    }
    (head.to_string(), inner.map(str::to_string))
}

/// A C# type (plus an object id from `[NavObjectId]`) as an AL variable type.
fn var_type(ty: &[Token], object_id: Option<i64>) -> VarType {
    let (name, inner) = type_name(ty);
    match name.as_str() {
        "NavRecordHandle" | "INavRecordHandle" => VarType::Record {
            table: object_id,
            temporary: None,
        },
        "NavCodeunitHandle" | "INavCodeunitHandle" => VarType::Object {
            kind: "Codeunit",
            id: object_id,
        },
        "NavFormHandle" => VarType::Object {
            kind: "Page",
            id: object_id,
        },
        "NavReportHandle" => VarType::Object {
            kind: "Report",
            id: object_id,
        },
        "NavQueryHandle" => VarType::Object {
            kind: "Query",
            id: object_id,
        },
        "NavXmlPortHandle" => VarType::Object {
            kind: "XmlPort",
            id: object_id,
        },
        "NavInterfaceHandle" => VarType::Interface,
        "NavList" => VarType::Plain(format!(
            "List of [{}]",
            inner.as_deref().and_then(plain_type).unwrap_or("Text")
        )),
        "NavDictionary" => VarType::Plain("Dictionary of [Text, Text]".into()),
        n => plain_type(n).map_or(VarType::Unknown, |p| VarType::Plain(p.into())),
    }
}

/// A C# parameter's AL type (`[NavObjectId]` giving a record's table).
pub(crate) fn param_var_type(p: &Param) -> VarType {
    var_type(&p.ty, p.object_id())
}

/// The AL type of a C# runtime type name alone (`bool`, `NavText`).
pub(crate) fn named_var_type(name: &str) -> VarType {
    plain_type(name).map_or(VarType::Unknown, |p| VarType::Plain(p.into()))
}

/// `new NavRecordHandle(scope, 18, false, …)` / `new NavCodeunitHandle(scope, 80)`
/// in `toks`: the handle type, the object id and the temporary flag.
fn handle_init(toks: &[Token]) -> Option<(String, i64, Option<bool>)> {
    let p = toks.iter().position(|t| t.is_ident("new"))?;
    let name = toks.get(p + 1)?.ident()?.to_string();
    if !name.starts_with("Nav") || !name.ends_with("Handle") {
        return None;
    }
    let open = toks.iter().skip(p).position(|t| t.is_punct("("))? + p;
    let close = matching(toks, open);
    let args = split_commas(&toks[open + 1..close - 1]);
    let id = int_literal(args.get(1)?)?;
    let temporary = args.get(2).and_then(|a| match a.as_slice() {
        [t] if t.is_ident("true") => Some(true),
        [t] if t.is_ident("false") => Some(false),
        _ => None,
    });
    Some((name, id, temporary))
}

fn int_literal(run: &[Token]) -> Option<i64> {
    match run {
        [
            Token {
                tok: Tok::Num(n), ..
            },
        ] => n.parse().ok(),
        [
            m,
            Token {
                tok: Tok::Num(n), ..
            },
        ] if m.is_punct("-") => n.parse::<i64>().ok().map(|v| -v),
        _ => None,
    }
}

/// `x = new …Handle(…)` with `x` the assignment target `γscope.Target.x` or
/// `this.x`: (`x`, init).
fn assigned_handle(run: &[Token]) -> Option<(String, String, i64, Option<bool>)> {
    let eq = run.iter().position(|t| t.is_punct("="))?;
    let target = run[..eq].last()?.ident()?.to_string();
    let (ty, id, temp) = handle_init(&run[eq + 1..])?;
    Some((target, ty, id, temp))
}

fn narrow(v: &mut VarType, handle: &str, id: i64, temp: Option<bool>) {
    *v = match handle {
        "NavRecordHandle" => VarType::Record {
            table: Some(id),
            temporary: temp,
        },
        "NavCodeunitHandle" => VarType::Object {
            kind: "Codeunit",
            id: Some(id),
        },
        "NavFormHandle" => VarType::Object {
            kind: "Page",
            id: Some(id),
        },
        "NavReportHandle" => VarType::Object {
            kind: "Report",
            id: Some(id),
        },
        "NavQueryHandle" => VarType::Object {
            kind: "Query",
            id: Some(id),
        },
        "NavXmlPortHandle" => VarType::Object {
            kind: "XmlPort",
            id: Some(id),
        },
        _ => return,
    };
}

fn walk<'a>(stmts: &'a [Stmt], f: &mut impl FnMut(&'a [Token])) {
    for s in stmts {
        match s {
            Stmt::Expr(t) | Stmt::Return(t) | Stmt::Throw(t) => f(t),
            Stmt::Local { init, .. } => f(init),
            Stmt::If { cond, then, els } => {
                f(cond);
                walk(then, f);
                if let Some(e) = els {
                    walk(e, f);
                }
            }
            Stmt::Loop { head, body, .. } => {
                f(head);
                walk(body, f);
            }
            Stmt::Switch { scrutinee, arms } => {
                f(scrutinee);
                for a in arms {
                    walk(a, f);
                }
            }
            Stmt::Try {
                body,
                catches,
                finally,
            } => {
                walk(body, f);
                for c in catches {
                    walk(c, f);
                }
                walk(finally, f);
            }
            Stmt::Block(b) => walk(b, f),
            _ => {}
        }
    }
}

/// The class's fields as AL globals, narrowed by their initializers in any
/// method (the constructor writes `this.cust = new NavRecordHandle(this, 18, …)`).
pub(crate) fn class_globals(class: &Class) -> HashMap<String, VarType> {
    let mut out = HashMap::new();
    for m in &class.members {
        if let Member::Field { ty, name, init, .. } = m {
            let mut v = var_type(ty, None);
            if let Some((h, id, temp)) = handle_init(init) {
                narrow(&mut v, &h, id, temp);
            }
            // A global the C# exposes as a property is stored in a backing
            // field `δx`; the code reads it as `this.x`.
            let name = name.strip_prefix('δ').unwrap_or(name);
            out.insert(name.to_string(), v);
        }
    }
    for m in class.methods() {
        walk(&m.body, &mut |run| {
            if run.first().is_some_and(|t| t.is_ident("this"))
                && let Some((name, h, id, temp)) = assigned_handle(run)
                && let Some(v) = out.get_mut(&name)
            {
                narrow(v, &h, id, temp);
            }
        });
    }
    out
}

/// A method's locals: parameters, then its `ALMethodScope<(…)>` tuple fields,
/// narrowed by their initializers.
pub(crate) fn method_locals(m: &Method) -> HashMap<String, VarType> {
    let mut out = HashMap::new();
    for p in &m.params {
        out.insert(p.name.clone(), var_type(&p.ty, p.object_id()));
    }
    // The scope tuple: `ALMethodScope<(T a, U b)> γscope = default;`
    let mut scope_tuple: Option<Vec<Token>> = None;
    walk(&m.body, &mut |_| {});
    for s in &m.body {
        if let Stmt::Local { ty, .. } = s
            && ty.first().is_some_and(|t| t.is_ident("ALMethodScope"))
        {
            scope_tuple = Some(ty.clone());
        }
    }
    if let Some(ty) = scope_tuple
        && let Some(open) = ty.iter().position(|t| t.is_punct("("))
    {
        let close = matching(&ty, open);
        for field in split_commas(&ty[open + 1..close - 1]) {
            let Some(name) = field.last().and_then(Token::ident) else {
                continue;
            };
            if name.starts_with('γ') {
                continue;
            }
            // A parameter keeps its object id.
            out.entry(name.to_string())
                .or_insert_with(|| var_type(&field[..field.len() - 1], None));
        }
    }
    walk(&m.body, &mut |run| {
        if run
            .first()
            .is_some_and(|t| t.ident().is_some_and(|s| s.starts_with('γ')))
            && let Some((name, h, id, temp)) = assigned_handle(run)
            && let Some(v) = out.get_mut(&name)
        {
            narrow(v, &h, id, temp);
        }
    });
    out
}

/// What a call's receiver is.
enum Receiver<'a> {
    /// A local or parameter (`γscope.Target.x…`).
    Local(&'a str),
    /// A class field (`this.x…`).
    Field(&'a str),
    /// The object itself: a table's own record (`this.ALModifyAsync`), or an
    /// own-method call (`Foo(…)`, `this.Foo(…)`).
    This,
    /// A page's `Rec`, `xRec`, `CurrPage`.
    PageMember(&'a str),
    /// A static class (`ALDatabase.ALCommit`).
    Static(&'a str),
    /// The method scope itself (`γscope.ALStart()`, `βscope.…`): plumbing.
    Scope,
}

/// A call chain's receiver segments: the method dropped, and the scope
/// plumbing (`Target`, `Value`).
fn receiver_segs(chain: &[String]) -> Vec<&str> {
    chain[..chain.len() - 1]
        .iter()
        .map(String::as_str)
        .filter(|s| *s != "Target" && *s != "Value")
        .collect()
}

fn is_scope(s: &str) -> bool {
    s.starts_with('γ') || s.starts_with('β')
}

/// The members between a variable receiver and the method, as AL calls:
/// `webResponse.ALContent.ALReadAs` reads `Content()` of `webResponse`. `None`
/// when one of them is C# without an AL name; empty for a direct call.
fn member_path(chain: &[String]) -> Option<Vec<String>> {
    let segs = receiver_segs(chain);
    let rest: &[&str] = match segs.as_slice() {
        [s, _, rest @ ..] if is_scope(s) => rest,
        ["this", _, rest @ ..] => rest,
        [first, rest @ ..] if first.starts_with(char::is_lowercase) && !is_scope(first) => rest,
        _ => &[],
    };
    rest.iter()
        .map(|m| al_method_name(m).map(|n| format!("{n}()")))
        .collect()
}

fn receiver(chain: &[String]) -> Receiver<'_> {
    let segs = receiver_segs(chain);
    let scope = is_scope;
    match segs.as_slice() {
        [] => Receiver::This,
        [s] if scope(s) => Receiver::Scope,
        [s, x, ..] if scope(s) => Receiver::Local(x),
        ["this"] | ["base"] => Receiver::This,
        ["this", m, ..]
            if matches!(
                *m,
                "Rec" | "xRec" | "CurrPage" | "CurrReport" | "CurrXmlPort"
            ) =>
        {
            Receiver::PageMember(m)
        }
        ["this", f, ..] => Receiver::Field(f),
        // A plain C# local outside the scope tuple (a lambda's, a `foreach`
        // variable): an untyped variable, never a static class.
        [first, ..] if first.starts_with(char::is_lowercase) => Receiver::Local(first),
        [first, ..] => Receiver::Static(first),
    }
}

/// Static classes whose calls never have an effect (string, number, date,
/// type-conversion and runtime-plumbing helpers).
fn is_pure_static(class: &str, method: &str) -> bool {
    match class {
        "ALCompiler"
        | "ALSystemString"
        | "NavTextExtensions"
        | "ALSystemDate"
        | "ALSystemNumeric"
        | "ALSystemArray"
        | "ALSystemVariable"
        | "ALSystemErrorHandling"
        | "ALNavApp"
        | "ALSessionInformation"
        | "NavRuntimeHelpers"
        | "ALMethodScope"
        | "NavType"
        | "NavOption"
        | "NavText"
        | "NavCode"
        | "NavDateTime"
        | "NavDate"
        | "NavTime"
        | "NavGuid"
        | "NavFormatEvaluateHelper"
        | "NavVariant"
        | "NavDecimal"
        | "NavBigInteger"
        | "ALOrdinals"
        | "ALNames"
        | "ALId"
        | "System"
        | "Math"
        | "string"
        | "String"
        | "Array"
        | "Enumerable"
        | "Guid"
        | "NavSecretText"
        | "NavALErrorInfo"
        | "NavModuleInfo"
        | "ALSystemLanguage"
        | "NavBigText"
        | "NavTextBuilder"
        | "Convert"
        | "Encoding"
        | "NavDuration"
        | "NavDateFormula"
        | "NavRecordId"
        | "NavJsonObject"
        | "NavJsonArray"
        | "NavJsonToken"
        | "NavJsonValue"
        | "NavInStream"
        | "NavOutStream"
        | "NavMedia"
        | "NavMediaSet"
        | "ALCryptographyManagement"
        | "ALTypeHelper"
        | "NavEnvironment"
        | "ALSystemConversion"
        | "NavDialogHandle"
        | "NavHttpClient"
        | "NavHttpRequestMessage"
        | "NavHttpResponseMessage"
        | "NavHttpContent"
        | "NavHttpHeaders"
        | "NavList"
        | "NavDictionary"
        | "NavArray"
        | "NavNotification"
        | "NavRecordRef"
        | "NavFieldRef"
        | "NavKeyRef"
        | "ByRef"
        | "ALStream"
        | "ALCompanyProperty" => true,
        "ALTaskScheduler" => matches!(method, "ALCanCreateTask" | "ALTaskExists"),
        c if c.starts_with("NavXml") => true,
        "ALDatabase" => !matches!(
            method,
            "ALCommit" | "ALSelectLatestVersion" | "ALLockTimeout"
        ),
        "ALSession" => matches!(
            method,
            "ALBindSubscription"
                | "ALUnbindSubscription"
                | "ALGetExecutionContext"
                | "ALGetModuleExecutionContext"
                | "ALGetCurrentModuleExecutionContext"
                | "ALIsSessionActive"
                | "ALCurrentClientType"
                | "ALDefaultClientType"
                | "ALCurrentExecutionMode"
                | "ALGetCallerExecutionContext"
                | "ALSessionId"
                | "ALServiceInstanceId"
                | "ALTenantId"
                | "ALApplicationIdentifier"
                | "ALEnableVerboseTelemetry"
                | "ALSetDocumentServiceToken"
        ),
        _ => false,
    }
}

/// Instance methods (not `AL`-prefixed, or AL ones with no effect worth
/// keeping) dropped on any receiver: assignment, comparison, conversion.
const PURE_INSTANCE: &[&str] = &[
    "ALAssign",
    "ALClose",
    "Clear",
    "Equals",
    "CompareTo",
    "ToString",
    "ToInt32",
    "ToInt64",
    "GetHashCode",
    "ArrayLen",
    "GetFieldValueSafe",
    "SetFieldValueSafe",
    "GetFieldRefSafe",
    "ClearFieldValue",
    "ALByValue",
    "ObjectID",
    "GetPart",
    "Dispose",
    "AsValueTaskObject",
    "ConfigureAwait",
    "GetTranslation",
    "InitializeComponent",
    "Contains",
    "Add",
    "Remove",
    "ContainsKey",
    "TryGetValue",
    "GetValue",
    "SetValue",
    "Get",
    "Set",
    "Invoke",
    "CreateInstance",
    "GetColumnValueSafe",
    "Value",
    "ALFieldCaptionAsync",
    "ALFieldCaption",
    "ALFieldName",
    "ALTableCaption",
    "ALTableName",
    "ALGetFilters",
    "ALGetFilter",
    "ALGetRangeMin",
    "ALGetRangeMax",
    "ALGetView",
    "ALSetView",
    "ALHasFilter",
    "ALFieldActive",
    "ALMarkedOnly",
    "ALMark",
    "ALCurrentKey",
    "ALAscending",
    "ALCurrentCompany",
    "ALGetPosition",
    "ALSetPosition",
    "ALRecordId",
    "ALSystemId",
    "ALCountApprox",
    "ALIsTemporary",
    "ALFieldNo",
    "ALClearMarks",
];

/// Record methods that read or write the database, raise an error, or run
/// trigger code: their AL name. Other record methods (filters, keys, field
/// access, `Init`, `Copy`, `TransferFields`, …) have no effect al-sem reads.
fn record_op(method: &str) -> Option<&'static str> {
    Some(match method {
        "ALInsertAsync" | "ALInsert" => "Insert",
        "ALModifyAsync" | "ALModify" => "Modify",
        "ALModifyAllSafeAsync" | "ALModifyAllAsync" | "ALModifyAll" => "ModifyAll",
        "ALDeleteAsync" | "ALDelete" => "Delete",
        "ALDeleteAllAsync" | "ALDeleteAll" => "DeleteAll",
        "ALRenameAsync" | "ALRename" => "Rename",
        "ALGetAsync" | "ALGet" | "ALGetBySystemIdAsync" | "ALGetBySystemId" => "Get",
        "ALFindSetAsync" | "ALFindSet" => "FindSet",
        "ALFindFirstAsync" | "ALFindFirst" => "FindFirst",
        "ALFindLastAsync" | "ALFindLast" => "FindLast",
        "ALFindAsync" | "ALFind" => "Find",
        "GetALIsEmptyAsync" | "ALIsEmptyAsync" | "ALIsEmpty" => "IsEmpty",
        "GetALCountAsync" | "ALCountAsync" | "ALCount" => "Count",
        "ALCalcFieldsAsync" | "ALCalcFields" => "CalcFields",
        "ALCalcSumsAsync" | "ALCalcSums" => "CalcSums",
        "ALTestFieldSafe" | "ALTestField" => "TestField",
        "ALFieldErrorAsync" | "ALFieldError" => "FieldError",
        "ALLockTable" | "ALLockTableAsync" => "LockTable",
        _ => return None,
    })
}

/// The AL method name of an `AL`-prefixed runtime method (`ALPostAsync` → `Post`).
fn al_method_name(method: &str) -> Option<String> {
    let m = method.strip_prefix("AL")?;
    let m = m.strip_suffix("Async").unwrap_or(m);
    let m = m.strip_suffix("Safe").unwrap_or(m);
    (!m.is_empty()).then(|| m.to_string())
}

/// What one method translates to.
#[derive(Debug, Default)]
pub(crate) struct AlBody {
    /// Local variables to declare: (name, AL type).
    pub(crate) locals: Vec<(String, String)>,
    /// Statement lines, each ending in `// cs <line>` where it came from one.
    pub(crate) lines: Vec<String>,
}

/// The object the method belongs to, as far as calls need it.
pub(crate) struct ObjectCx<'a> {
    /// `Codeunit`, `Table`, `Page`, … (AL keyword).
    pub(crate) kind: &'a str,
    /// The class's methods by C# name: their method ids (several for C#
    /// overloads).
    pub(crate) methods_by_cs_name: HashMap<&'a str, Vec<(usize, Option<i32>)>>,
    /// The class's event publishers (`[NavEvent(…)]`) by C# name, which is the
    /// AL name: an event cannot be overloaded, so a raise calls it by name.
    pub(crate) events: std::collections::HashSet<&'a str>,
    pub(crate) globals: &'a HashMap<String, VarType>,
}

impl<'a> ObjectCx<'a> {
    /// The context of `class`'s methods: `kind` is the object's AL keyword.
    pub(crate) fn of(
        class: &'a Class,
        kind: &'a str,
        globals: &'a HashMap<String, VarType>,
    ) -> Self {
        let mut methods_by_cs_name: HashMap<&str, Vec<(usize, Option<i32>)>> = HashMap::new();
        let mut events = std::collections::HashSet::new();
        for m in class.methods() {
            if m.attribute("NavEvent").is_some() {
                events.insert(m.name.as_str());
            }
            methods_by_cs_name
                .entry(m.name.as_str())
                .or_default()
                .push((m.params.len(), m.method_id()));
        }
        ObjectCx {
            kind,
            methods_by_cs_name,
            events,
            globals,
        }
    }
}

/// The placeholder local every condition reads.
pub(crate) const BRANCH: &str = "\"alsem branch\"";
/// The placeholder a dynamic object id reads.
const DYN_INT: &str = "\"alsem int\"";
/// The undeclared procedure an untranslatable call becomes.
pub(crate) const UNTRANSLATED: &str = "\"__alsem_untranslated\"";

struct Tx<'a> {
    cx: &'a ObjectCx<'a>,
    locals: &'a HashMap<String, VarType>,
    lines: Vec<String>,
    depth: usize,
    used_int: bool,
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', ""))
}

impl Tx<'_> {
    fn push(&mut self, stmt: &str, line: u32) {
        let pad = "    ".repeat(self.depth);
        if line > 0 {
            self.lines.push(format!("{pad}{stmt} // cs {line}"));
        } else {
            self.lines.push(format!("{pad}{stmt}"));
        }
    }

    fn var(&self, r: &Receiver) -> Option<(String, VarType)> {
        match r {
            Receiver::Local(x) => self.locals.get(*x).map(|v| (quote(x), v.clone())),
            Receiver::Field(f) => self.cx.globals.get(*f).map(|v| (quote(f), v.clone())),
            _ => None,
        }
    }

    /// An argument as AL: a variable passed through, a literal kept, anything
    /// else a placeholder (`0`).
    fn arg(&self, run: &[Token]) -> String {
        match run {
            [
                Token {
                    tok: Tok::Num(n), ..
                },
            ] => return n.clone(),
            [
                m,
                Token {
                    tok: Tok::Num(n), ..
                },
            ] if m.is_punct("-") => return format!("-{n}"),
            [
                Token {
                    tok: Tok::Str(_), ..
                },
            ] => return "''".into(),
            [t] if t.is_ident("true") || t.is_ident("false") => {
                return t.ident().unwrap_or("").into();
            }
            _ => {}
        }
        // One variable reference inside (an unwrap of `ALCompiler.ToNavValue(x)`,
        // `new ByRef<T>(() => x, …)`): pass it.
        let mut refs: Vec<String> = Vec::new();
        for (i, t) in run.iter().enumerate() {
            let Some(id) = t.ident() else { continue };
            let after = |k: usize| run.get(i + k);
            if id.starts_with('γ')
                && after(1).is_some_and(|t| t.is_punct("."))
                && after(2).is_some_and(|t| t.is_ident("Target"))
            {
                if let Some(x) = after(4).and_then(Token::ident)
                    && self.locals.contains_key(x)
                {
                    refs.push(quote(x));
                }
            } else if id == "this"
                && after(1).is_some_and(|t| t.is_punct("."))
                && let Some(f) = after(2).and_then(Token::ident)
                && self.cx.globals.contains_key(f)
            {
                refs.push(quote(f));
            }
        }
        refs.dedup();
        if let [one] = refs.as_slice() {
            return one.clone();
        }
        // No variable: one literal inside a conversion (`new NavText("x")`,
        // `ALCompiler.ToNavValue(1)`) is that literal.
        if refs.is_empty() {
            let lits: Vec<String> = run
                .iter()
                .enumerate()
                .filter_map(|(i, t)| match &t.tok {
                    Tok::Num(n) if i > 0 && run[i - 1].is_punct("-") => Some(format!("-{n}")),
                    Tok::Num(n) => Some(n.clone()),
                    Tok::Str(_) => Some("''".into()),
                    _ => None,
                })
                .collect();
            if let [one] = lits.as_slice() {
                return one.clone();
            }
        }
        "0".into()
    }

    /// The AL arguments of a runtime call, `DataError` and session plumbing
    /// dropped.
    fn args(&self, args: &[Vec<Token>]) -> Vec<String> {
        args.iter()
            .filter(|a| !is_plumbing(a))
            .map(|a| self.arg(a))
            .collect()
    }

    /// `"__alsem_mid_<n>"` on `target` (empty: a bare call).
    /// `args`: the AL call's own arguments (an `InvokeAsync`'s object array,
    /// [`args_of_invoke`], or a direct call's).
    fn mid_call(&self, target: &str, id: i32, args: &[Vec<Token>]) -> String {
        let a = self.args(args);
        let call = format!("\"__alsem_mid_{id}\"({})", a.join(", "));
        if target.is_empty() {
            call
        } else {
            format!("{target}.{call}")
        }
    }

    fn untranslated(&mut self, shape: &str, line: u32) {
        self.push(
            &format!("{UNTRANSLATED}('{}');", shape.replace('\'', "")),
            line,
        );
    }

    /// The AL statement(s) for one call, or nothing for a pure one.
    fn call(&mut self, c: &Call, in_condition: bool) {
        if c.is_new {
            return;
        }
        let method = c.method();
        let line = c.line;
        let trap = c
            .args
            .iter()
            .any(|a| a.iter().any(|t| t.is_ident("TrapError")));
        let consumed = |s: String| format!("if {s} then;");
        let r = receiver(&c.chain);
        match r {
            Receiver::Static(class) => self.static_call(class, method, c, line),
            Receiver::Scope => {}
            Receiver::This | Receiver::PageMember(_) => {
                let target = match r {
                    Receiver::PageMember(m) => m.to_string(),
                    _ => String::new(),
                };
                if target == "CurrPage" {
                    match method {
                        "UpdateAsync" | "Update" => self.push("CurrPage.Update();", line),
                        "Close" => self.push("CurrPage.Close();", line),
                        "SaveRecordAsync" | "SaveRecord" => {
                            self.push("CurrPage.SaveRecord();", line)
                        }
                        _ => {}
                    }
                    return;
                }
                // The table's own record, or a page's `Rec`/`xRec`.
                let rec_target = if target.is_empty() {
                    "Rec".to_string()
                } else {
                    target.clone()
                };
                if self.cx.kind == "Table" || !target.is_empty() {
                    if let Some(op) = record_op(method) {
                        let s = format!("{rec_target}.{op}({});", self.args(&c.args).join(", "));
                        self.push(
                            &if trap {
                                consumed(s.trim_end_matches(';').into())
                            } else {
                                s
                            },
                            line,
                        );
                        return;
                    }
                    if method == "InvokeAsync" && !target.is_empty() {
                        if let Some(id) = c.args.first().and_then(|a| int_literal(a)) {
                            let s = self.mid_call(&rec_target, id as i32, &args_of_invoke(&c.args));
                            self.push(&format!("{s};"), line);
                        }
                        return;
                    }
                    if method == "ALValidateSafeAsync" || method == "ALValidate" {
                        self.untranslated("Validate", line);
                        return;
                    }
                }
                if !target.is_empty() {
                    return;
                }
                // An event raise: the publisher by name.
                if self.cx.events.contains(method) {
                    let a = self.args(&c.args).join(", ");
                    self.push(&format!("{}({a});", quote(method)), line);
                    return;
                }
                // An own-method call.
                if let Some(cands) = self.cx.methods_by_cs_name.get(method) {
                    let fitting: Vec<i32> = cands
                        .iter()
                        .filter(|(arity, _)| *arity == c.args.len())
                        .filter_map(|(_, id)| *id)
                        .collect();
                    match fitting.as_slice() {
                        [id] => {
                            let s = self.mid_call("", *id, &c.args); // a direct call
                            self.push(&format!("{s};"), line);
                        }
                        _ => self.untranslated(&format!("own call {method}"), line),
                    }
                }
            }
            Receiver::Local(_) | Receiver::Field(_) => {
                let Some((name, ty)) = self.var(&r) else {
                    if !PURE_INSTANCE.contains(&method) && method.starts_with("AL") {
                        self.untranslated(&format!("untyped .{method}"), line);
                    }
                    return;
                };
                match ty {
                    VarType::Record { .. } => {
                        if let Some(op) = record_op(method) {
                            let s = format!("{name}.{op}({})", self.args(&c.args).join(", "));
                            self.push(
                                &if trap
                                    || in_condition
                                        && op != "Insert"
                                        && op != "Modify"
                                        && op != "Delete"
                                {
                                    consumed(s)
                                } else {
                                    format!("{s};")
                                },
                                line,
                            );
                        } else if method == "InvokeAsync" {
                            if let Some(id) = c.args.first().and_then(|a| int_literal(a)) {
                                let s = self.mid_call(&name, id as i32, &args_of_invoke(&c.args));
                                self.push(&format!("{s};"), line);
                            }
                        } else if method == "ALValidateSafeAsync" || method == "ALValidate" {
                            self.untranslated("Validate", line);
                        }
                    }
                    VarType::Object { kind, .. } => match method {
                        "InvokeAsync" => match c.args.first().and_then(|a| int_literal(a)) {
                            Some(id) => {
                                let s = self.mid_call(&name, id as i32, &args_of_invoke(&c.args));
                                self.push(&format!("{s};"), line);
                            }
                            None => self.untranslated(&format!("{kind} dynamic invoke"), line),
                        },
                        "RunModalAsync" | "RunModal" => {
                            self.push(&format!("{name}.RunModal();"), line)
                        }
                        "RunAsync" | "Run" => self.push(&format!("{name}.Run();"), line),
                        m if PURE_INSTANCE.contains(&m) => {}
                        m => {
                            if let Some(al) = al_method_name(m) {
                                let a = self.args(&c.args).join(", ");
                                self.push(&format!("{name}.{al}({a});"), line);
                            }
                        }
                    },
                    VarType::Interface => {
                        if method == "InvokeAsync" {
                            self.untranslated("interface dispatch", line);
                        }
                    }
                    VarType::Plain(ty) => {
                        if PURE_INSTANCE.contains(&method) {
                            return;
                        }
                        if let Some(al) = al_method_name(method) {
                            let path = member_path(&c.chain);
                            // An option or enum the C# shows as `NavOption`
                            // (declared `Integer`): its members (`Names()`,
                            // `Ordinals()`, then the list's) read metadata only.
                            if ty == "Integer" && path.as_ref().is_none_or(|p| !p.is_empty()) {
                                return;
                            }
                            let Some(path) = path else {
                                self.untranslated(&format!("member chain .{method}"), line);
                                return;
                            };
                            // Runtime names that differ from AL's:
                            // `HttpContent.WriteFrom`, `RecordRef.FieldExist`.
                            let al = match al.as_str() {
                                "LoadFrom" => "WriteFrom".into(),
                                "FieldExists" => "FieldExist".into(),
                                _ => al,
                            };
                            let target = std::iter::once(name)
                                .chain(path)
                                .collect::<Vec<_>>()
                                .join(".");
                            let a = self.args(&c.args).join(", ");
                            self.push(&format!("{target}.{al}({a});"), line);
                        }
                    }
                    VarType::Unknown => {
                        if !PURE_INSTANCE.contains(&method) && method.starts_with("AL") {
                            self.untranslated(&format!("untyped .{method}"), line);
                        }
                    }
                }
            }
        }
    }

    fn static_call(&mut self, class: &str, method: &str, c: &Call, line: u32) {
        let trap = c
            .args
            .iter()
            .any(|a| a.iter().any(|t| t.is_ident("TrapError")));
        let num = c.args.iter().find_map(|a| int_literal(a));
        match (class, method) {
            ("ALDatabase", "ALCommit") => self.push("Commit();", line),
            ("ALDatabase", "ALSelectLatestVersion") => self.push("SelectLatestVersion();", line),
            ("ALDatabase", "ALLockTimeout") => {}
            ("NavDialog", "ALError") => self.push("Error('');", line),
            ("NavDialog", "ALMessage") => self.push("Message('');", line),
            ("NavDialog", "ALConfirm") => self.push("if Confirm('') then;", line),
            ("NavDialog", "ALStrMenu") => self.push("if StrMenu('') = 0 then;", line),
            ("ALSystemOperatingSystem", "ALHyperlink") => self.push("Hyperlink('');", line),
            ("NavForm", "RunAsync" | "Run") => match num {
                Some(id) => self.push(&format!("Page.Run({id});"), line),
                None => {
                    self.used_int = true;
                    self.push(&format!("Page.Run({DYN_INT});"), line);
                }
            },
            ("NavForm", "RunModalAsync" | "RunModal") => match num {
                Some(id) => self.push(&format!("if Page.RunModal({id}) = Action::LookupOK then;"), line),
                None => {
                    self.used_int = true;
                    self.push(&format!("if Page.RunModal({DYN_INT}) = Action::LookupOK then;"), line);
                }
            },
            ("NavReport", m) if m.starts_with("Run") => match num {
                Some(id) => self.push(&format!("Report.Run({id});"), line),
                None => {
                    self.used_int = true;
                    self.push(&format!("Report.Run({DYN_INT});"), line);
                }
            },
            ("NavCodeunit", "RunCodeunitAsync" | "RunCodeunit") => {
                let id = c.args.iter().skip(2).find_map(|a| int_literal(a));
                let target = match id {
                    Some(id) => id.to_string(),
                    None => {
                        self.used_int = true;
                        DYN_INT.to_string()
                    }
                };
                // A record argument (`Codeunit.Run(X, Rec)`) passes through.
                let rec = c.args.get(3).map(|a| self.arg(a)).filter(|a| a != "0");
                let call = match rec {
                    Some(r) => format!("Codeunit.Run({target}, {r})"),
                    None => format!("Codeunit.Run({target})"),
                };
                self.push(&if trap { format!("if {call} then;") } else { format!("{call};") }, line);
            }
            ("ALIsolatedStorage", m) => {
                if let Some(al) = al_method_name(m) {
                    self.push(&format!("IsolatedStorage.{al}('', DataScope::Module);"), line);
                }
            }
            ("ALTaskScheduler", m) if m.starts_with("ALCancelTask") => {
                self.push("TaskScheduler.CancelTask(CreateGuid());", line)
            }
            ("ALTaskScheduler", m) if m.starts_with("ALCreateTask") => match num {
                Some(id) => self.push(&format!("TaskScheduler.CreateTask({id}, 0);"), line),
                None => {
                    self.used_int = true;
                    self.push(&format!("TaskScheduler.CreateTask({DYN_INT}, 0);"), line);
                }
            },
            ("ALSession", "ALStartSession" | "ALStartSessionAsync") => match num {
                Some(id) => {
                    self.used_int = true;
                    self.push(&format!("StartSession({DYN_INT}, {id});"), line);
                }
                None => self.untranslated("StartSession dynamic", line),
            },
            ("ALSession", "ALLogMessage" | "ALLogSecurityAudit") => self.push(
                "Session.LogMessage('', '', Verbosity::Normal, DataClassification::SystemMetadata, TelemetryScope::All, '', '');",
                line,
            ),
            ("ALFile" | "NavFile", m) => {
                if let Some(al) = al_method_name(m) {
                    self.push(&format!("File.{al}();"), line);
                }
            }
            ("ALSystemFile" | "ALFileManagement", m) if m.contains("Upload") || m.contains("Download") => {
                let al = al_method_name(m).unwrap_or_else(|| m.to_string());
                self.push(&format!("{al}();"), line);
            }
            (cls, m) if is_pure_static(cls, m) => {}
            (cls, m) if m.starts_with("AL") || cls.starts_with("AL") || cls.starts_with("Nav") => {
                self.untranslated(&format!("{cls}.{m}"), line);
            }
            _ => {}
        }
    }

    /// Every call in a token run, in order.
    fn run(&mut self, toks: &[Token], in_condition: bool) {
        for c in calls(toks) {
            self.call(&c, in_condition);
        }
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        self.depth += 1;
        self.stmts(stmts);
        self.depth -= 1;
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expr(t) => self.run(t, false),
            Stmt::Local { ty, init, .. } => {
                // The scope declaration and its `new ALMethodScope(…)` are plumbing.
                if !ty.first().is_some_and(|t| t.is_ident("ALMethodScope")) {
                    self.run(init, false);
                }
            }
            Stmt::Return(t) => {
                self.run(t, false);
                self.push("exit;", first_line(t));
            }
            Stmt::Throw(_) | Stmt::Break | Stmt::Continue | Stmt::Goto | Stmt::Label => {}
            Stmt::If { cond, then, els } => {
                self.run(cond, true);
                self.push(&format!("if {BRANCH} then begin"), first_line(cond));
                self.block(then);
                match els {
                    Some(e) => {
                        self.push("end else begin", 0);
                        self.block(e);
                        self.push("end;", 0);
                    }
                    None => self.push("end;", 0),
                }
            }
            Stmt::Loop {
                head,
                body,
                post_test,
            } => {
                if *post_test {
                    self.push("repeat", first_line(head));
                    self.block(body);
                    self.depth += 1;
                    self.run(head, true);
                    self.depth -= 1;
                    self.push(&format!("until {BRANCH};"), 0);
                } else {
                    self.run(head, true);
                    self.push(&format!("while {BRANCH} do begin"), first_line(head));
                    self.block(body);
                    self.push("end;", 0);
                }
            }
            Stmt::Switch { scrutinee, arms } => {
                self.run(scrutinee, true);
                for (k, arm) in arms.iter().enumerate() {
                    let kw = if k == 0 { "if" } else { "end else if" };
                    self.push(&format!("{kw} {BRANCH} then begin"), first_line(scrutinee));
                    self.block(arm);
                }
                if !arms.is_empty() {
                    self.push("end;", 0);
                }
            }
            Stmt::Try { body, .. } => {
                // The method's own `try { … } catch … finally { ALFinally }` is
                // plumbing: its body is the routine. The catch rethrows.
                self.stmts(body);
            }
            Stmt::Block(b) => self.stmts(b),
        }
    }
}

/// `DataError.X`, `this.Session`, `γscope`, `this`, a `Guid.Parse(…)` label id:
/// arguments of the runtime, not of the AL call.
fn is_plumbing(run: &[Token]) -> bool {
    match run {
        [a, dot, _] if a.is_ident("DataError") && dot.is_punct(".") => true,
        [a, dot, s] if a.is_ident("this") && dot.is_punct(".") && s.is_ident("Session") => true,
        [a] if a.is_ident("this") || a.ident().is_some_and(|s| s.starts_with('γ')) => true,
        [a, ..] if a.is_ident("System") || a.is_ident("Guid") => true,
        _ => false,
    }
}

/// `InvokeAsync(id, new object[] { a, b })`: the call's own arguments.
fn args_of_invoke(args: &[Vec<Token>]) -> Vec<Vec<Token>> {
    let Some(arr) = args.get(1) else {
        return Vec::new();
    };
    if arr.iter().any(|t| t.is_ident("Empty")) {
        return Vec::new();
    }
    match arr.iter().position(|t| t.is_punct("{")) {
        Some(open) => {
            let close = matching(arr, open);
            split_commas(&arr[open + 1..close - 1])
        }
        None => Vec::new(),
    }
}

/// Translate one method into an AL body.
pub(crate) fn translate(cx: &ObjectCx, m: &Method) -> AlBody {
    let locals = method_locals(m);
    let params: Vec<&str> = m.params.iter().map(|p: &Param| p.name.as_str()).collect();
    let mut tx = Tx {
        cx,
        locals: &locals,
        lines: Vec::new(),
        depth: 0,
        used_int: false,
    };
    tx.stmts(&m.body);
    let mut body = AlBody {
        lines: tx.lines,
        ..AlBody::default()
    };
    let mut names: Vec<&String> = locals
        .keys()
        .filter(|n| !params.contains(&n.as_str()))
        .collect();
    names.sort();
    for n in names {
        if let Some(t) = locals[n].al_type() {
            body.locals.push((quote(n), t));
        }
    }
    body.locals.push((BRANCH.to_string(), "Boolean".into()));
    if tx.used_int {
        body.locals.push((DYN_INT.to_string(), "Integer".into()));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::super::cs::{classes, lex};
    use super::*;

    const SAMPLE: &str = r#"
public sealed class Codeunit50100 : NavCodeunit
{
    [NavName("Log")]
    private NavRecordHandle log;

    public Codeunit50100(ITreeObject parent) : base(parent, 50100)
    {
        this.log = new NavRecordHandle(this, 50150, false, SecurityFiltering.Validated);
    }

    [NavName("DoWork")]
    [MethodId(-12345)]
    public async ValueTask DoWork([NavObjectId(ObjectId = 18)][NavByReferenceAttribute] INavRecordHandle rec)
    {
        ALMethodScope<(INavRecordHandle rec, NavRecordHandle temp, NavCodeunitHandle other, NavHttpClient client, bool γfiller)> γscope = default;
        try
        {
            γscope = new ALMethodScope<(INavRecordHandle rec, NavRecordHandle temp, NavCodeunitHandle other, NavHttpClient client, bool γfiller)>(this, "DoWork", MethodScopeFlags.None, -12345);
            γscope.Target.temp = new NavRecordHandle(γscope, 50150, true, SecurityFiltering.Validated);
            γscope.Target.other = new NavCodeunitHandle(γscope, 50101);
            {
                if (γscope.CStmtHit(0) & ((await γscope.Target.rec.Target.ALGetAsync(DataError.TrapError, ALCompiler.ToNavValue(1)))))
                {
                    γscope.StmtHit(1);
                    await γscope.Target.rec.Target.ALModifyAsync(DataError.ThrowError, true);
                    γscope.StmtHit(2);
                    return;
                }
                γscope.StmtHit(3);
                await γscope.Target.temp.Target.ALInsertAsync(DataError.ThrowError);
                await this.log.Target.ALInsertAsync(DataError.ThrowError);
                ALDatabase.ALCommit();
                await γscope.Target.other.Target.InvokeAsync(42, new object[] { γscope.Target.rec, new NavText("x") });
                await γscope.Target.client.ALPostAsync(DataError.ThrowError, new NavText("u"), ALCompiler.ToNavValue(1), ALCompiler.ToNavValue(2));
                await Helper();
                ALWeird.ALDoSomething();
                string s = ALSystemString.ALStrSubstNo("x");
            }
        }
        catch (Exception αe)when (ALMethodScope.ALException(γscope, αe, out NavALException αale))
        {
            throw αale;
        }
        finally
        {
            ALMethodScope.ALFinally(γscope);
        }
    }

    [NavName("Helper")]
    [MethodId(777)]
    public async ValueTask Helper()
    {
    }
}
"#;

    fn sample_body() -> AlBody {
        let classes = classes(&lex(SAMPLE));
        let class = &classes[0];
        let globals = class_globals(class);
        let cx = ObjectCx::of(class, "Codeunit", &globals);
        let m = class.methods().find(|m| m.name == "DoWork").unwrap();
        translate(&cx, m)
    }

    #[test]
    fn translates_runtime_calls_back_to_al() {
        let body = sample_body();
        let text: Vec<String> = body
            .lines
            .iter()
            .map(|l| l.split(" // cs").next().unwrap().trim().to_string())
            .collect();
        assert_eq!(
            text,
            [
                "if \"rec\".Get(1) then;",
                "if \"alsem branch\" then begin",
                "\"rec\".Modify(true);",
                "exit;",
                "end;",
                "\"temp\".Insert();",
                "\"log\".Insert();",
                "Commit();",
                "\"other\".\"__alsem_mid_42\"(\"rec\", '');",
                "\"client\".Post('', 1, 2);",
                "\"__alsem_mid_777\"();",
                "\"__alsem_untranslated\"('ALWeird.ALDoSomething');",
            ]
        );
        assert!(
            // The `ALGetAsync` sits on SAMPLE's line 23 (line 1 is the empty
            // line after `r#"`).
            body.lines[0].ends_with("// cs 23"),
            "{:?}",
            body.lines[0]
        );
    }

    /// Gated on `TRANSPILED_CS_DIR` (a real package's `bin/*.cs`, never
    /// committed): translate every method and print how many statements
    /// stayed `__alsem_untranslated`, by shape.
    #[test]
    fn census_of_a_real_package_when_given_one() {
        let Some(dir) = std::env::var_os("TRANSPILED_CS_DIR") else {
            return;
        };
        let mut shapes: HashMap<String, usize> = HashMap::new();
        let (mut methods, mut lines) = (0usize, 0usize);
        for e in std::fs::read_dir(dir).expect("dir") {
            let p = e.expect("entry").path();
            if p.extension().is_none_or(|x| x != "cs") {
                continue;
            }
            let text = std::fs::read_to_string(&p).expect("read");
            for class in classes(&lex(&text)) {
                let globals = class_globals(&class);
                let kind = if class.name.starts_with("Record") {
                    "Table"
                } else if class.name.starts_with("Page") {
                    "Page"
                } else {
                    "Codeunit"
                };
                let cx = ObjectCx::of(&class, kind, &globals);
                for m in class.methods().filter(|m| m.method_id().is_some()) {
                    methods += 1;
                    let body = translate(&cx, m);
                    lines += body.lines.len();
                    for l in &body.lines {
                        if let Some(rest) = l.split(UNTRANSLATED).nth(1) {
                            let shape = rest.split('\'').nth(1).unwrap_or("?").to_string();
                            *shapes.entry(shape).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut v: Vec<_> = shapes.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        eprintln!(
            "methods {methods} lines {lines} untranslated {}",
            v.iter().map(|x| x.1).sum::<usize>()
        );
        for (s, n) in v.iter().take(40) {
            eprintln!("{n:6} {s}");
        }
    }

    /// A generic type's comma (`NavDictionary<NavText, NavText>`) splits no
    /// scope local or argument; a property between the variable and the method
    /// (`resp.ALContent.ALReadAs`) is kept as an AL call; `ALLoadFrom` is
    /// `WriteFrom`; a C# member with no AL name leaves the call untranslated; an
    /// option's member chain (`kind.ALNames.ALIndexOf`) reads metadata only.
    #[test]
    fn keeps_generic_types_and_member_paths() {
        const SRC: &str = r#"
public sealed class Codeunit50102 : NavCodeunit
{
    private NavDialog δwin;

    [NavName("Send")]
    [MethodId(5)]
    public async ValueTask Send()
    {
        ALMethodScope<(NavDictionary<NavText, NavText> dict, NavHttpClient client, NavHttpContent content, NavHttpResponseMessage resp, NavText body, NavOption kind, NavRecordRef rr)> γscope = default;
        γscope = new ALMethodScope<(NavDictionary<NavText, NavText> dict, NavHttpClient client, NavHttpContent content, NavHttpResponseMessage resp, NavText body, NavOption kind, NavRecordRef rr)>(this, "Send", MethodScopeFlags.None, 5);
        γscope.Target.rr.ALFieldExists(51);
        await this.win.ALOpenAsync<NavValue>(System.Guid.Parse("7cdc0a34-000b-0003-0507-0b0d1113171d"), msg);
        γscope.Target.kind.Value.ALNames.ALIndexOf(new NavText(γscope.Target.body));
        γscope.Target.dict.ALAdd(DataError.ThrowError, new NavText("a"), γscope.Target.body);
        γscope.Target.content.ALLoadFrom(γscope.Target.body);
        γscope.Target.client.Value.ALDefaultRequestHeaders.ALAdd(DataError.ThrowError, new NavText("h"), new NavText("v"));
        γscope.Target.resp.ALContent.ALReadAs(DataError.ThrowError, new ByRef<NavText>(() => γscope.Target.body, setValue => γscope.Target.body = setValue));
        γscope.Target.resp.Inner.ALReadAs(DataError.ThrowError, new ByRef<NavText>(() => γscope.Target.body, setValue => γscope.Target.body = setValue));
    }
}
"#;
        let classes = classes(&lex(SRC));
        let globals = class_globals(&classes[0]);
        let cx = ObjectCx::of(&classes[0], "Codeunit", &globals);
        let body = translate(&cx, classes[0].methods().next().unwrap());
        let text: Vec<String> = body
            .lines
            .iter()
            .map(|l| l.split(" // cs").next().unwrap().trim().to_string())
            .collect();
        assert_eq!(
            text,
            [
                "\"rr\".FieldExist(51);",
                "\"win\".Open(0);",
                "\"dict\".Add('', \"body\");",
                "\"content\".WriteFrom(\"body\");",
                "\"client\".DefaultRequestHeaders().Add('', '');",
                "\"resp\".Content().ReadAs(\"body\");",
                "\"__alsem_untranslated\"('member chain .ALReadAs');",
            ]
        );
        let dict = body.locals.iter().find(|(n, _)| n == "\"dict\"");
        assert_eq!(
            dict.map(|(_, t)| t.as_str()),
            Some("Dictionary of [Text, Text]")
        );
    }

    #[test]
    fn declares_locals_with_their_tables_and_temporariness() {
        let body = sample_body();
        let get = |n: &str| {
            body.locals
                .iter()
                .find(|(name, _)| name == n)
                .map(|(_, t)| t.as_str())
        };
        assert_eq!(get("\"temp\""), Some("Record 50150 temporary"));
        assert_eq!(get("\"other\""), Some("Codeunit 50101"));
        assert_eq!(get("\"client\""), Some("HttpClient"));
        assert_eq!(get("\"rec\""), None, "a parameter is not a local");
        let classes = classes(&lex(SAMPLE));
        let g = class_globals(&classes[0]);
        assert_eq!(
            g.get("log"),
            Some(&VarType::Record {
                table: Some(50150),
                temporary: Some(false)
            })
        );
    }
}
