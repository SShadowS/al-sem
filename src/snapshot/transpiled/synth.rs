//! AL source for every object of a runtime package: declarations from its
//! `SymbolReference.json`, routine bodies from its transpiled C# ([`translate`]).
//!
//! An object becomes one AL file, `transpiled/<Kind><id>.al`. Declarations keep
//! what al-sem resolves against: object ids and names (and namespaces), table
//! fields with their types, classes, option members and keys, enum values with
//! their implementations, page source tables, extension targets, and every
//! routine's signature and event attributes. Captions, permissions and the like
//! are left out.
//!
//! Soundness: a routine the symbols declare whose C# body cannot be found gets a
//! body that is one untranslated call, never an empty body (which would claim
//! the routine does nothing). A C# routine the symbols do not declare (a local
//! event subscriber) is declared from its C# signature.

use super::cs::{Class, Method, Tok, classes, lex};
use super::translate::{ObjectCx, UNTRANSLATED, VarType, class_globals, translate};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write;

fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', ""))
}

fn prop<'a>(o: &'a Value, name: &str) -> Option<&'a str> {
    o.get("Properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("Name").and_then(Value::as_str) == Some(name))?
        .get("Value")?
        .as_str()
}

fn arr<'a>(o: &'a Value, key: &str) -> &'a [Value] {
    o.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn s<'a>(o: &'a Value, key: &str) -> &'a str {
    o.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A `TypeDefinition` as AL type text, `None` for a type that cannot be
/// declared here (`DotNet`).
pub(crate) fn type_text(td: &Value) -> Option<String> {
    let name = s(td, "Name");
    let base = match name {
        "" => return None,
        "DotNet" => return None,
        "Label" => "Label ''".to_string(),
        "Option" => {
            let members: Vec<String> = arr(td, "OptionMembers")
                .iter()
                .filter_map(Value::as_str)
                .map(|m| {
                    if m.is_empty() || m.chars().any(|c| !c.is_alphanumeric() && c != '_') {
                        q(m)
                    } else {
                        m.to_string()
                    }
                })
                .collect();
            if members.is_empty() {
                "Option".to_string()
            } else {
                format!("Option {}", members.join(","))
            }
        }
        "List" | "Dictionary" => {
            let args: Vec<String> = arr(td, "TypeArguments")
                .iter()
                .map(|a| type_text(a).unwrap_or_else(|| "Text".into()))
                .collect();
            if args.is_empty() {
                return None;
            }
            format!("{name} of [{}]", args.join(", "))
        }
        _ => match td.get("Subtype") {
            Some(st) => {
                let id = st.get("Id").and_then(Value::as_i64);
                let sub_name = s(st, "Name");
                match (name, id) {
                    ("Interface", _) | (_, None) if !sub_name.is_empty() => {
                        format!("{name} {}", q(sub_name.trim_matches('"')))
                    }
                    (_, Some(id)) => format!("{name} {id}"),
                    _ => name.to_string(),
                }
            }
            None => name.to_string(),
        },
    };
    let base = if td.get("Temporary").and_then(Value::as_bool) == Some(true) {
        format!("{base} temporary")
    } else {
        base
    };
    let dims: Vec<String> = arr(td, "ArrayDimensions")
        .iter()
        .filter_map(Value::as_i64)
        .map(|d| d.to_string())
        .collect();
    Some(if dims.is_empty() {
        base
    } else {
        format!("array[{}] of {base}", dims.join(", "))
    })
}

/// The AL kind and file prefix of a symbol list, and the C# class prefix.
const KINDS: &[(&str, &str, &str, &str)] = &[
    // (symbol list, AL keyword, file prefix, C# class prefix)
    ("Tables", "table", "TAB", "Record"),
    ("Codeunits", "codeunit", "COD", "Codeunit"),
    ("Pages", "page", "PAG", "Page"),
    ("PageExtensions", "pageextension", "PEX", "PageExtension"),
    ("Reports", "report", "REP", "Report"),
    ("XmlPorts", "xmlport", "XML", "XmlPort"),
    ("Queries", "query", "QUE", "Query"),
    ("EnumTypes", "enum", "ENU", ""),
    ("EnumExtensionTypes", "enumextension", "ENX", ""),
    ("Interfaces", "interface", "INT", ""),
    ("ControlAddIns", "controladdin", "CAI", ""),
    ("TableExtensions", "tableextension", "TEX", "TableExtension"),
];

/// A C# class name split into its prefix and object id (`Record18` → (`Record`, 18)).
fn class_key(name: &str) -> Option<(&str, i64)> {
    let i = name.find(|c: char| c.is_ascii_digit())?;
    Some((&name[..i], name[i..].parse().ok()?))
}

/// The target name of an extension (`#<app guid>#Payment Journal`).
fn target_name(o: &Value) -> String {
    let t = s(o, "TargetObject");
    t.rsplit('#').next().unwrap_or(t).to_string()
}

struct Out {
    text: String,
}

impl Out {
    fn line(&mut self, depth: usize, l: &str) {
        let _ = writeln!(self.text, "{}{l}", "    ".repeat(depth));
    }
}

/// A routine to write: its header and, from the C#, its body.
struct Routine<'a> {
    attributes: Vec<String>,
    access: &'static str,
    name: String,
    params: Vec<String>,
    ret: Option<String>,
    cs: Option<&'a Method>,
    /// An event publisher or an interface method: no body to find.
    bodyless: bool,
    interface: bool,
}

fn symbol_attributes(m: &Value) -> Vec<String> {
    arr(m, "Attributes")
        .iter()
        .filter_map(|a| {
            let name = s(a, "Name");
            if !matches!(
                name,
                "IntegrationEvent" | "BusinessEvent" | "InternalEvent" | "TryFunction"
            ) {
                return None;
            }
            let args: Vec<String> = arr(a, "Arguments")
                .iter()
                .map(|v| s(v, "Value").to_ascii_lowercase())
                .collect();
            Some(if args.is_empty() {
                format!("[{name}]")
            } else {
                format!("[{name}({})]", args.join(", "))
            })
        })
        .collect()
}

/// The `[MethodId('n')]` attribute a synthesized routine carries
/// (`decl_surface::declared_method_id` reads it). The id is quoted because the
/// grammar takes no negative number as an attribute argument: `[MethodId(-5)]`
/// made the whole file `Recovered`, and every object in it parse-incomplete.
fn method_id_attribute(id: i32) -> String {
    format!("[MethodId('{id}')]")
}

/// `[NavEventSubscriber(ObjectType.CodeUnit, 80, "OnX", id, "Elem", opts)]` as AL.
fn subscriber_attribute(m: &Method) -> Option<String> {
    let a = m.attribute("NavEventSubscriber")?;
    let ty = a.args.first()?.last()?.ident()?.to_string();
    let ty = match ty.to_ascii_lowercase().as_str() {
        "codeunit" => "Codeunit",
        "table" => "Table",
        "page" => "Page",
        "report" => "Report",
        "xmlport" => "XmlPort",
        "query" => "Query",
        _ => return None,
    };
    let id = a.args.get(1).and_then(|r| match r.as_slice() {
        [t] => match &t.tok {
            Tok::Num(n) => n.parse::<i64>().ok(),
            _ => None,
        },
        _ => None,
    })?;
    let strs: Vec<&str> = a.str_args();
    let event = strs.first()?;
    let element = strs.get(1).copied().unwrap_or("");
    let skip_license = a.mentions("SkipOnMissingLicense");
    let skip_permission = a.mentions("SkipOnMissingPermission");
    Some(format!(
        "[EventSubscriber(ObjectType::{ty}, {id}, '{event}', '{element}', {skip_license}, {skip_permission})]"
    ))
}

/// A C# parameter as an AL parameter (a routine the symbols do not declare).
/// A type with no AL counterpart here is `Variant`: the routine stays declared
/// (its body analysed), only its method id may then not match a call by id.
fn cs_param(p: &super::cs::Param) -> Option<String> {
    let ty = super::translate::param_var_type(p)
        .al_type()
        .unwrap_or_else(|| "Variant".into());
    let var = if p.by_reference() || p.ty.iter().any(|t| t.is_ident("ByRef")) {
        "var "
    } else {
        ""
    };
    Some(format!("{var}{}: {ty}", q(&p.name)))
}

/// A C# return type (`ValueTask<bool>`) as AL, `None` for none.
fn cs_return(m: &Method) -> Option<String> {
    let inner: Vec<&str> = m.ret.iter().filter_map(|t| t.ident()).collect();
    let last = inner.last().copied()?;
    if matches!(last, "ValueTask" | "Task" | "void") {
        return None;
    }
    match super::translate::named_var_type(last) {
        VarType::Plain(p) => Some(p),
        _ => None,
    }
}

struct ObjectWriter<'a> {
    out: Out,
    class: Option<&'a Class>,
    globals: HashMap<String, VarType>,
    kind: &'static str,
}

impl ObjectWriter<'_> {
    fn cx(&self) -> Option<ObjectCx<'_>> {
        let kind = match self.kind {
            "table" => "Table",
            "page" | "pageextension" => "Page",
            _ => "Codeunit",
        };
        self.class.map(|c| ObjectCx::of(c, kind, &self.globals))
    }

    /// One routine's body lines (with its `var` section), at `depth`.
    fn body(&mut self, depth: usize, cs: Option<&Method>, bodyless: bool) {
        let Some(m) = cs else {
            self.out.line(depth, "begin");
            if !bodyless {
                self.out
                    .line(depth + 1, &format!("{UNTRANSLATED}('no transpiled body');"));
            }
            self.out.line(depth, "end;");
            return;
        };
        let body = match self.cx() {
            Some(cx) => translate(&cx, m),
            None => return,
        };
        if !body.locals.is_empty() {
            self.out.line(depth, "var");
            for (n, t) in &body.locals {
                self.out.line(depth + 1, &format!("{n}: {t};"));
            }
        }
        self.out.line(depth, "begin");
        for l in &body.lines {
            self.out.line(depth + 1, l);
        }
        self.out.line(depth, "end;");
    }

    fn routine(&mut self, depth: usize, r: &Routine) {
        for a in &r.attributes {
            self.out.line(depth, a);
        }
        let ret = r.ret.as_ref().map_or(String::new(), |t| format!(": {t}"));
        let header = format!(
            "{}procedure {}({}){ret}",
            r.access,
            q(&r.name),
            r.params.join("; ")
        );
        if r.interface {
            self.out.line(depth, &format!("{header};"));
            return;
        }
        self.out.line(depth, &header);
        self.body(depth, r.cs, r.bodyless);
    }

    fn globals_section(&mut self, depth: usize) {
        let mut names: Vec<(&String, String)> = self
            .globals
            .iter()
            .filter_map(|(n, v)| v.al_type().map(|t| (n, t)))
            .collect();
        names.sort();
        let lines: Vec<String> = names
            .iter()
            .map(|(n, t)| format!("{}: {t};", q(n)))
            .collect();
        if !lines.is_empty() {
            self.out.line(depth, "var");
            for l in lines {
                self.out.line(depth + 1, &l);
            }
        }
    }
}

/// The routines of object `o` (symbols) matched with `class` (C#) by method id,
/// plus the C# routines the symbols do not declare (not triggers).
fn routines<'a>(o: &Value, class: Option<&'a Class>, interface: bool) -> Vec<Routine<'a>> {
    let cs_by_id: HashMap<i32, &Method> = class
        .map(|c| {
            c.methods()
                .filter(|m| !m.is_trigger())
                .filter_map(|m| Some((m.method_id()?, m)))
                .collect()
        })
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in arr(o, "Methods") {
        let id = m
            .get("Id")
            .and_then(Value::as_i64)
            .and_then(|i| i32::try_from(i).ok());
        let cs = id.and_then(|i| cs_by_id.get(&i).copied());
        if let Some(i) = id {
            seen.insert(i);
        }
        let mut attributes = symbol_attributes(m);
        let is_event = attributes.iter().any(|a| a.contains("Event("));
        // The id the runtime calls it by (`"__alsem_mid_<n>"` calls bind to
        // it): computing it from the signature fails for an enum parameter
        // the C# shows only as `NavOption`.
        attributes.extend(id.map(method_id_attribute));
        let sym_params = arr(m, "Parameters");
        // C# names by position, so the body's references match.
        let names: Vec<String> = match cs {
            Some(c) if c.params.len() == sym_params.len() => {
                c.params.iter().map(|p| p.name.clone()).collect()
            }
            _ => sym_params
                .iter()
                .map(|p| s(p, "Name").to_string())
                .collect(),
        };
        let params: Vec<String> = sym_params
            .iter()
            .zip(&names)
            .map(|(p, n)| {
                let ty = p
                    .get("TypeDefinition")
                    .and_then(type_text)
                    .unwrap_or_else(|| "Variant".into());
                let var = if p.get("IsVar").and_then(Value::as_bool) == Some(true) {
                    "var "
                } else {
                    ""
                };
                format!("{var}{}: {ty}", q(n))
            })
            .collect();
        let access = if m.get("IsLocal").and_then(Value::as_bool) == Some(true) {
            "local "
        } else if m.get("IsInternal").and_then(Value::as_bool) == Some(true) {
            "internal "
        } else if m.get("IsProtected").and_then(Value::as_bool) == Some(true) {
            "protected "
        } else {
            ""
        };
        out.push(Routine {
            attributes,
            access,
            name: s(m, "Name").to_string(),
            params,
            ret: m.get("ReturnTypeDefinition").and_then(type_text),
            cs,
            bodyless: is_event,
            interface,
        });
    }
    if let Some(c) = class {
        for m in c.methods().filter(|m| !m.is_trigger()) {
            let Some(id) = m.method_id() else { continue };
            if seen.contains(&id) {
                continue;
            }
            let Some(name) = m.nav_name() else { continue };
            let params: Option<Vec<String>> = m.params.iter().map(cs_param).collect();
            let Some(params) = params else { continue };
            out.push(Routine {
                attributes: subscriber_attribute(m)
                    .into_iter()
                    .chain(std::iter::once(method_id_attribute(id)))
                    .collect(),
                access: "local ",
                name: name.to_string(),
                params,
                ret: cs_return(m),
                cs: Some(m),
                bodyless: false,
                interface: false,
            });
        }
    }
    out
}

/// An object trigger: its name (`OnRun`) and C# method.
type ObjectTrigger<'a> = (&'a str, &'a Method);
/// A member trigger: the member (`E-Mail`), the trigger (`OnValidate`), the C# method.
type MemberTrigger<'a> = (&'a str, &'a str, &'a Method);

/// The C# triggers of `class` by placement: object triggers (`OnRun`,
/// `OnInsert`, `OnOpenPage`, …) and member triggers (`<Field> - OnValidate`,
/// `<Action> - OnAction`) by member name.
fn triggers(class: Option<&Class>) -> (Vec<ObjectTrigger<'_>>, Vec<MemberTrigger<'_>>) {
    let mut object = Vec::new();
    let mut member = Vec::new();
    for m in class
        .into_iter()
        .flat_map(Class::methods)
        .filter(|m| m.is_trigger())
    {
        let Some(n) = m.nav_name() else { continue };
        match n.rsplit_once(" - ") {
            Some((member_name, trig)) => member.push((member_name, trig, m)),
            None => object.push((n, m)),
        }
    }
    (object, member)
}

fn trigger_params(m: &Method) -> String {
    m.params
        .iter()
        .filter_map(cs_param)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Write one object, or `None` for a kind this module does not write.
fn write_object(
    list: &str,
    kw: &'static str,
    o: &Value,
    namespace: &str,
    class: Option<&Class>,
) -> Option<String> {
    let id = o.get("Id").and_then(Value::as_i64);
    let name = s(o, "Name");
    let mut w = ObjectWriter {
        out: Out {
            text: String::new(),
        },
        class,
        globals: class.map(class_globals).unwrap_or_default(),
        kind: kw,
    };
    if !namespace.is_empty() {
        w.out.line(0, &format!("namespace {namespace};"));
        w.out.line(0, "");
    }
    w.out.line(
        0,
        "// Synthesized by al-sem from this package's symbols and its transpiled C# (bin/*.cs).",
    );
    let header = match (kw, id) {
        ("interface" | "controladdin", _) => format!("{kw} {}", q(name)),
        ("pageextension" | "enumextension" | "tableextension", Some(id)) => {
            format!("{kw} {id} {} extends {}", q(name), q(&target_name(o)))
        }
        (_, Some(id)) => format!("{kw} {id} {}", q(name)),
        _ => return None,
    };
    let header = if kw == "enum" {
        let ifaces: Vec<&str> = arr(o, "ImplementedInterfaces")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if ifaces.is_empty() {
            header
        } else {
            format!("{header} implements {}", ifaces.join(", "))
        }
    } else {
        header
    };
    w.out.line(0, &header);
    w.out.line(0, "{");
    let (object_triggers, member_triggers) = triggers(class);
    match list {
        "Tables" | "TableExtensions" => {
            if let Some(t) = prop(o, "TableType") {
                w.out.line(1, &format!("TableType = {t};"));
            }
            w.out.line(1, "fields");
            w.out.line(1, "{");
            for f in arr(o, "Fields") {
                let fname = s(f, "Name");
                let ty = f
                    .get("TypeDefinition")
                    .and_then(type_text)
                    .unwrap_or_else(|| "Blob".into());
                // An option field declares its members as a property.
                let (ty, members) = match ty.strip_prefix("Option ") {
                    Some(m) => ("Option".to_string(), Some(m.to_string())),
                    None => (ty, None),
                };
                w.out.line(
                    2,
                    &format!(
                        "field({}; {}; {ty})",
                        f.get("Id").and_then(Value::as_i64).unwrap_or(0),
                        q(fname)
                    ),
                );
                w.out.line(2, "{");
                if let Some(m) = members {
                    w.out.line(3, &format!("OptionMembers = {m};"));
                }
                if let Some(fc) = prop(f, "FieldClass") {
                    w.out.line(3, &format!("FieldClass = {fc};"));
                }
                if let Some(cf) = prop(f, "CalcFormula") {
                    w.out.line(3, &format!("CalcFormula = {cf};"));
                }
                for (_, trig, m) in member_triggers
                    .iter()
                    .filter(|(mn, _, _)| mn.eq_ignore_ascii_case(fname))
                {
                    w.out
                        .line(3, &format!("trigger {trig}({})", trigger_params(m)));
                    w.body(3, Some(m), false);
                }
                w.out.line(2, "}");
            }
            w.out.line(1, "}");
            let keys = arr(o, "Keys");
            if !keys.is_empty() {
                w.out.line(1, "keys");
                w.out.line(1, "{");
                for k in keys {
                    let fields: Vec<String> = arr(k, "FieldNames")
                        .iter()
                        .filter_map(Value::as_str)
                        .map(q)
                        .collect();
                    w.out.line(
                        2,
                        &format!("key({}; {}) {{ }}", q(s(k, "Name")), fields.join(", ")),
                    );
                }
                w.out.line(1, "}");
            }
        }
        "Codeunits" => {
            for p in [
                "SingleInstance",
                "EventSubscriberInstance",
                "Subtype",
                "TableNo",
            ] {
                if let Some(v) = prop(o, p) {
                    let v = match (p, v) {
                        ("SingleInstance", "1") => "true",
                        ("SingleInstance", "0") => "false",
                        _ => v,
                    };
                    w.out.line(1, &format!("{p} = {v};"));
                }
            }
        }
        "Pages" | "PageExtensions" => {
            if let Some(v) = prop(o, "PageType") {
                w.out.line(1, &format!("PageType = {v};"));
            }
            if let Some(v) = prop(o, "SourceTable") {
                w.out.line(1, &format!("SourceTable = {v};"));
            }
            if prop(o, "SourceTableTemporary")
                .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            {
                w.out.line(1, "SourceTableTemporary = true;");
            }
            // Control triggers: an action's `OnAction` in `actions`, every other
            // control trigger on a field of the page's own placeholder.
            let (actions, fields): (Vec<_>, Vec<_>) = member_triggers
                .iter()
                .partition(|(_, trig, _)| trig.eq_ignore_ascii_case("OnAction"));
            if !fields.is_empty() && kw == "page" {
                w.out.line(1, "layout");
                w.out.line(1, "{");
                w.out.line(2, "area(Content)");
                w.out.line(2, "{");
                for (member, trig, m) in &fields {
                    w.out
                        .line(3, &format!("field({}; \"alsem page field\")", q(member)));
                    w.out.line(3, "{");
                    w.out
                        .line(4, &format!("trigger {trig}({})", trigger_params(m)));
                    w.body(4, Some(m), false);
                    w.out.line(3, "}");
                }
                w.out.line(2, "}");
                w.out.line(1, "}");
                w.globals
                    .insert("alsem page field".into(), VarType::Plain("Text".into()));
            }
            if !actions.is_empty() && kw == "page" {
                w.out.line(1, "actions");
                w.out.line(1, "{");
                w.out.line(2, "area(Processing)");
                w.out.line(2, "{");
                for (member, _, m) in &actions {
                    w.out.line(3, &format!("action({})", q(member)));
                    w.out.line(3, "{");
                    w.out.line(4, "trigger OnAction()");
                    w.body(4, Some(m), false);
                    w.out.line(3, "}");
                }
                w.out.line(2, "}");
                w.out.line(1, "}");
            }
            if kw == "pageextension" {
                // An extension's member triggers stay reachable as procedures.
                for (member, trig, m) in &member_triggers {
                    w.out.line(
                        1,
                        &format!(
                            "local procedure {}()",
                            q(&format!("alsem {member} - {trig}"))
                        ),
                    );
                    w.body(1, Some(m), false);
                }
            }
        }
        "EnumTypes" | "EnumExtensionTypes" => {
            if let Some(v) = prop(o, "Extensible") {
                let v = if v == "1" || v.eq_ignore_ascii_case("true") {
                    "true"
                } else {
                    "false"
                };
                w.out.line(1, &format!("Extensible = {v};"));
            }
            let ifaces: Vec<&str> = arr(o, "ImplementedInterfaces")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            for v in arr(o, "Values") {
                let ord = v.get("Ordinal").and_then(Value::as_i64).unwrap_or(0);
                w.out.line(1, &format!("value({ord}; {})", q(s(v, "Name"))));
                w.out.line(1, "{");
                if let (Some(imp), [iface]) = (prop(v, "Implementation"), ifaces.as_slice()) {
                    w.out.line(2, &format!("Implementation = {iface} = {imp};"));
                }
                w.out.line(1, "}");
            }
        }
        "Queries" => {
            w.out.line(1, "elements");
            w.out.line(1, "{");
            w.out.line(1, "}");
        }
        _ => {}
    }
    // Object triggers.
    for (n, m) in &object_triggers {
        w.out
            .line(1, &format!("trigger {n}({})", trigger_params(m)));
        w.body(1, Some(m), false);
    }
    // A table's member triggers whose field is not declared stay as procedures.
    if list == "Tables" {
        let fields: Vec<&str> = arr(o, "Fields").iter().map(|f| s(f, "Name")).collect();
        for (member, trig, m) in member_triggers
            .iter()
            .filter(|(mn, _, _)| !fields.iter().any(|f| f.eq_ignore_ascii_case(mn)))
        {
            w.out.line(
                1,
                &format!(
                    "local procedure {}()",
                    q(&format!("alsem {member} - {trig}"))
                ),
            );
            w.body(1, Some(m), false);
        }
    }
    let interface = list == "Interfaces";
    let rs = if list == "ControlAddIns" {
        Vec::new()
    } else {
        routines(o, class, interface)
    };
    // Globals before routines (a page's placeholder field added above).
    if !interface {
        w.globals_section(1);
    }
    for r in &rs {
        w.routine(1, r);
    }
    w.out.line(0, "}");
    Some(w.out.text)
}

/// Synthesize one AL file per object of a package: `symbols` is its
/// `SymbolReference.json`, `cs` its `bin/*.cs` as (entry name, text).
/// Returns (virtual path, AL text) pairs.
pub(crate) fn synthesize(symbols: &Value, cs: &[(String, String)]) -> Vec<(String, String)> {
    let parsed: Vec<Class> = cs.iter().flat_map(|(_, t)| classes(&lex(t))).collect();
    let mut by_key: HashMap<(&str, i64), &Class> = HashMap::new();
    for c in parsed.iter().filter(|c| c.outer.is_none()) {
        if let Some(k) = class_key(&c.name) {
            by_key.entry(k).or_insert(c);
        }
    }
    let mut out = Vec::new();
    let mut stack: Vec<(&Value, String)> = vec![(symbols, String::new())];
    while let Some((node, ns)) = stack.pop() {
        for &(list, kw, prefix, cs_prefix) in KINDS {
            for o in arr(node, list) {
                let id = o.get("Id").and_then(Value::as_i64);
                let class = id.and_then(|i| by_key.get(&(cs_prefix, i)).copied());
                if let Some(text) = write_object(list, kw, o, &ns, class) {
                    let tag = id
                        .map_or_else(|| s(o, "Name").replace(['/', '\\'], "_"), |i| i.to_string());
                    out.push((format!("transpiled/{prefix}{tag}.al"), text));
                }
            }
        }
        for child in arr(node, "Namespaces") {
            let child_ns = if ns.is_empty() {
                s(child, "Name").to_string()
            } else {
                format!("{ns}.{}", s(child, "Name"))
            };
            stack.push((child, child_ns));
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A routine's method id is written so the file still parses clean when
    /// the id is negative (half of all ids are), and reads back as declared.
    #[test]
    fn a_negative_method_id_keeps_the_file_clean() {
        let symbols: Value = serde_json::from_str(
            r#"{ "Codeunits": [ { "Id": 6192855, "Name": "CSC DB Reader", "Methods": [
                { "Id": -324976780, "Name": "GetName" },
                { "Id": 798086828, "Name": "GetCreator" } ] } ] }"#,
        )
        .expect("json");
        let files = synthesize(&symbols, &[]);
        let [(_, text)] = files.as_slice() else {
            panic!("one file: {files:?}")
        };
        let f = al_syntax::parse(text);
        assert_eq!(f.parse_status, al_syntax::ir::ParseStatus::Clean, "{text}");
        let ids: Vec<Option<i32>> = f.objects[0]
            .routines
            .iter()
            .map(|r| {
                crate::program::resolve::decl_surface::RoutineMeta::from_decl(r, "C.al").method_id
            })
            .collect();
        assert_eq!(ids, vec![Some(-324976780), Some(798086828)], "{text}");
    }

    /// Gated on `TRANSPILED_SYMBOLS` (a real package's `SymbolReference.json`)
    /// and `TRANSPILED_CS_DIR` (its `bin/*.cs`), never committed; with
    /// `TRANSPILED_OUT` set, the synthesized files are written there. Prints
    /// how many files and routines parse clean.
    #[test]
    fn synthesizes_a_real_package_when_given_one() {
        let (Some(sym), Some(dir)) = (
            std::env::var_os("TRANSPILED_SYMBOLS"),
            std::env::var_os("TRANSPILED_CS_DIR"),
        ) else {
            return;
        };
        let symbols: Value =
            serde_json::from_str(&std::fs::read_to_string(sym).expect("symbols")).expect("json");
        let mut cs = Vec::new();
        for e in std::fs::read_dir(dir).expect("dir") {
            let p = e.expect("entry").path();
            if p.extension().is_some_and(|x| x == "cs") {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                cs.push((name, std::fs::read_to_string(&p).expect("read")));
            }
        }
        let files = synthesize(&symbols, &cs);
        let out = std::env::var_os("TRANSPILED_OUT").map(std::path::PathBuf::from);
        let (mut recovered, mut routines, mut incomplete) = (0, 0, 0);
        let mut shown = 0;
        for (path, text) in &files {
            if let Some(o) = &out {
                let p = o.join(path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, text).unwrap();
            }
            let f = al_syntax::parse(text);
            if f.parse_status != al_syntax::ir::ParseStatus::Clean {
                recovered += 1;
                if shown < 6 {
                    shown += 1;
                    eprintln!(
                        "{path}: {:?}",
                        f.issues
                            .iter()
                            .take(2)
                            .map(|i| &i.message)
                            .collect::<Vec<_>>()
                    );
                }
            }
            for o in &f.objects {
                routines += o.routines.len();
                incomplete += o.routines.iter().filter(|r| r.parse_incomplete).count();
            }
        }
        eprintln!(
            "files {} recovered {recovered} routines {routines} parse-incomplete {incomplete}",
            files.len()
        );
    }
}
