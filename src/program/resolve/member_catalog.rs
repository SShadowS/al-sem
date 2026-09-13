//! Clean-room MEMBER-builtin catalog for Phase 3 member-call resolution.
//!
//! Sourced from `tools/gen-al-builtins/out/member_builtins.json` (AL extension
//! ms-dynamics-smb.al-18.0.2293710).  Does NOT import from
//! `crate::engine::l3::member_builtins`.
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::program::resolve::member_catalog::{MemberCatalogKind, member_builtin, member_builtin_id};
//! use crate::program::resolve::receiver::FrameworkKind;
//!
//! let kind = FrameworkKind::JsonObject;
//! assert!(member_builtin(MemberCatalogKind::Framework(&kind), "add"));
//! let id = member_builtin_id(MemberCatalogKind::RecordRef, "field").unwrap();
//! assert_eq!(id.0, "RecordRef::field");
//! ```

use phf::phf_set;

use crate::program::resolve::edge::BuiltinId;
use crate::program::resolve::receiver::FrameworkKind;

// ---------------------------------------------------------------------------
// MemberCatalogKind
// ---------------------------------------------------------------------------

/// Discriminant passed to [`member_builtin`] and [`member_builtin_id`] to select
/// which catalog set to look up a method name against.
#[derive(Debug, Clone, Copy)]
pub enum MemberCatalogKind<'a> {
    RecordRef,
    FieldRef,
    KeyRef,
    Record,
    Framework(&'a FrameworkKind),
}

// ---------------------------------------------------------------------------
// Static phf sets — all lowercase, sourced from member_builtins.json
// ---------------------------------------------------------------------------

static RECORDREF: phf::Set<&'static str> = phf_set! {
    "addlink", "addloadfields", "arefieldsloaded", "ascending", "caption",
    "changecompany", "clearmarks", "close", "copy", "copylinks", "count",
    "countapprox", "currentcompany", "currentkey", "currentkeyindex", "delete",
    "deleteall", "deletelink", "deletelinks", "duplicate", "field", "fieldcount",
    "fieldexist", "fieldindex", "filtergroup", "find", "findfirst", "findlast",
    "findset", "fullyqualifiedname", "get", "getbysystemid", "getfilters",
    "getposition", "gettable", "getview", "hasfilter", "haslinks", "init",
    "insert", "isdirty", "isempty", "istemporary", "keycount", "keyindex",
    "loadfields", "locktable", "mark", "markedonly", "modify", "name", "next",
    "number", "open", "readconsistency", "readisolation", "readpermission",
    "recordid", "recordlevellocking", "rename", "reset", "securityfiltering",
    "setautocalcfields", "setloadfields", "setpermissionfilter", "setposition",
    "setrecfilter", "settable", "setview", "systemcreatedatno",
    "systemcreatedbyno", "systemidno", "systemmodifiedatno",
    "systemmodifiedbyno", "truncate", "writepermission"
};

static FIELDREF: phf::Set<&'static str> = phf_set! {
    "active", "calcfield", "calcsum", "caption", "class", "enumvaluecount",
    "fielderror", "getenumvaluecaption", "getenumvaluecaptionfromordinalvalue",
    "getenumvaluename", "getenumvaluenamefromordinalvalue",
    "getenumvalueordinal", "getfilter", "getrangemax", "getrangemin",
    "isenum", "isoptimizedfortextsearch", "length", "name", "number",
    "optioncaption", "optionmembers", "optionstring", "record", "relation",
    "setfilter", "setrange", "testfield", "type", "validate", "value"
};

static KEYREF: phf::Set<&'static str> = phf_set! {
    "active", "fieldcount", "fieldindex", "record"
};

static RECORD: phf::Set<&'static str> = phf_set! {
    "addlink", "addloadfields", "arefieldsloaded", "ascending", "calcfields",
    "calcsums", "changecompany", "clearmarks", "consistent", "copy",
    "copyfilter", "copyfilters", "copylinks", "count", "countapprox",
    "currentcompany", "currentkey", "delete", "deleteall", "deletelink",
    "deletelinks", "fieldactive", "fieldcaption", "fielderror", "fieldname",
    "fieldno", "filtergroup", "find", "findfirst", "findlast", "findset",
    "fullyqualifiedname", "get", "getascending", "getbysystemid", "getfilter",
    "getfilters", "getposition", "getrangemax", "getrangemin", "getview",
    "hasfilter", "haslinks", "init", "insert", "isempty", "istemporary",
    "loadfields", "locktable", "mark", "markedonly", "modify", "modifyall",
    "next", "readconsistency", "readisolation", "readpermission", "recordid",
    "recordlevellocking", "relation", "rename", "reset", "securityfiltering",
    "setascending", "setautocalcfields", "setbaseloadfields", "setcurrentkey",
    "setfilter", "setloadfields", "setpermissionfilter", "setposition",
    "setrange", "setrecfilter", "setview", "tablecaption", "tablename",
    "testfield", "transferfields", "truncate", "validate", "writepermission"
};

static JSONOBJECT: phf::Set<&'static str> = phf_set! {
    "add", "astoken", "clone", "contains", "get", "getarray", "getbiginteger",
    "getboolean", "getbyte", "getchar", "getdate", "getdatetime", "getdecimal",
    "getduration", "getinteger", "getobject", "getoption", "gettext", "gettime",
    "keys", "path", "readfrom", "readfromyaml", "remove", "replace",
    "selecttoken", "selecttokens", "values", "writeto", "writetoyaml",
    "writewithsecretsto"
};

static JSONTOKEN: phf::Set<&'static str> = phf_set! {
    "asarray", "asobject", "asvalue", "clone", "isarray", "isobject",
    "isvalue", "path", "readfrom", "selecttoken", "selecttokens", "writeto"
};

static JSONARRAY: phf::Set<&'static str> = phf_set! {
    "add", "astoken", "clone", "count", "get", "getarray", "getbiginteger",
    "getboolean", "getbyte", "getchar", "getdate", "getdatetime", "getdecimal",
    "getduration", "getinteger", "getobject", "getoption", "gettext", "gettime",
    "indexof", "insert", "path", "readfrom", "removeat", "selecttoken",
    "selecttokens", "set", "writeto"
};

static JSONVALUE: phf::Set<&'static str> = phf_set! {
    "asbiginteger", "asboolean", "asbyte", "aschar", "ascode", "asdate",
    "asdatetime", "asdecimal", "asduration", "asinteger", "asoption", "astext",
    "astime", "astoken", "clone", "isnull", "isundefined", "path", "readfrom",
    "selecttoken", "setvalue", "setvaluetonull", "setvaluetoundefined", "writeto"
};

static HTTPCLIENT: phf::Set<&'static str> = phf_set! {
    "addcertificate", "clear", "defaultrequestheaders", "delete", "get",
    "getbaseaddress", "patch", "post", "put", "send", "setbaseaddress",
    "timeout", "usedefaultnetworkwindowsauthentication", "useresponsecookies",
    "useservercertificatevalidation", "usewindowsauthentication"
};

static HTTPREQUEST: phf::Set<&'static str> = phf_set! {
    "content", "getcookie", "getcookienames", "getheaders", "getrequesturi",
    "getsecretrequesturi", "method", "removecookie", "setcookie",
    "setrequesturi", "setsecretrequesturi"
};

static HTTPRESPONSE: phf::Set<&'static str> = phf_set! {
    "content", "getcookie", "getcookienames", "headers", "httpstatuscode",
    "isblockedbyenvironment", "issuccessstatuscode", "reasonphrase"
};

static HTTPCONTENT: phf::Set<&'static str> = phf_set! {
    "clear", "getheaders", "issecretcontent", "readas", "writefrom"
};

static HTTPHEADERS: phf::Set<&'static str> = phf_set! {
    "add", "clear", "contains", "containssecret", "getsecretvalues",
    "getvalues", "keys", "remove", "tryaddwithoutvalidation"
};

static INSTREAM: phf::Set<&'static str> = phf_set! {
    "eos", "length", "position", "read", "readtext", "resetposition"
};

static OUTSTREAM: phf::Set<&'static str> = phf_set! {
    "write", "writetext"
};

static TEXTBUILDER: phf::Set<&'static str> = phf_set! {
    "append", "appendline", "capacity", "clear", "ensurecapacity", "insert",
    "length", "maxcapacity", "remove", "replace", "totext"
};

static TEXT: phf::Set<&'static str> = phf_set! {
    "contains", "convertstr", "copystr", "delchr", "delstr", "endswith",
    "incstr", "indexof", "indexofany", "insstr", "lastindexof", "lowercase",
    "maxstrlen", "padleft", "padright", "padstr", "remove", "replace",
    "selectstr", "split", "startswith", "strchecksum", "strlen", "strpos",
    "strsubstno", "substring", "tolower", "toupper", "trim", "trimend",
    "trimstart", "uppercase"
};

static BIGTEXT: phf::Set<&'static str> = phf_set! {
    "addtext", "getsubtext", "length", "read", "textpos", "write"
};

static SECRETTEXT: phf::Set<&'static str> = phf_set! {
    "isempty", "secretstrsubstno", "unwrap"
};

static LIST: phf::Set<&'static str> = phf_set! {
    "add", "addrange", "contains", "count", "get", "getrange", "indexof",
    "insert", "lastindexof", "remove", "removeat", "removerange", "reverse",
    "set"
};

static DICTIONARY: phf::Set<&'static str> = phf_set! {
    "add", "containskey", "count", "get", "keys", "remove", "set", "values"
};

static XML: phf::Set<&'static str> = phf_set! {
    "add", "addafterself", "addbeforeself", "addfirst", "addnamespace",
    "asxmlattribute", "asxmlcdata", "asxmlcomment", "asxmldeclaration",
    "asxmldocument", "asxmldocumenttype", "asxmlelement", "asxmlnode",
    "asxmlprocessinginstruction", "asxmltext", "attributes", "count", "create",
    "createnamespacedeclaration", "encoding", "get", "getchildelements",
    "getchildnodes", "getdata", "getdeclaration", "getdescendantelements",
    "getdescendantnodes", "getdocument", "getdocumenttype", "getinternalsubset",
    "getname", "getnamespaceofprefix", "getparent", "getprefixofnamespace",
    "getpublicid", "getroot", "getsystemid", "gettarget", "hasattributes",
    "haselements", "hasnamespace", "innertext", "innerxml", "isempty",
    "isnamespacedeclaration", "isxmlattribute", "isxmlcdata", "isxmlcomment",
    "isxmldeclaration", "isxmldocument", "isxmldocumenttype", "isxmlelement",
    "isxmlprocessinginstruction", "isxmltext", "localname", "lookupnamespace",
    "lookupprefix", "name", "nametable", "namespaceprefix", "namespaceuri",
    "popscope", "preservewhitespace", "pushscope", "readfrom", "remove",
    "removeall", "removeallattributes", "removeattribute", "removenodes",
    "removenamespace", "replacenodes", "replacewith", "selectnodes",
    "selectsinglenode", "set", "setattribute", "setdata", "setdeclaration",
    "setinternalsubset", "setname", "setpublicid", "setsystemid", "settarget",
    "standalone", "value", "version", "writeto"
};

static DATE: phf::Set<&'static str> = phf_set! {
    "day", "dayofweek", "month", "totext", "weekno", "year"
};

static DATETIME: phf::Set<&'static str> = phf_set! {
    "date", "time", "totext"
};

static TIME: phf::Set<&'static str> = phf_set! {
    "hour", "millisecond", "minute", "second", "totext"
};

static DURATION: phf::Set<&'static str> = phf_set! {
    "totext"
};

static GUID: phf::Set<&'static str> = phf_set! {
    "createguid", "createsequentialguid", "totext"
};

static BLOB: phf::Set<&'static str> = phf_set! {
    "createinstream", "createoutstream", "export", "hasvalue", "import",
    "length"
};

static MEDIA: phf::Set<&'static str> = phf_set! {
    "count", "exportfile", "exportstream", "findorphans", "hasvalue",
    "importfile", "importstream", "insert", "item", "mediaid", "remove"
};

static NOTIFICATION: phf::Set<&'static str> = phf_set! {
    "addaction", "getdata", "hasdata", "id", "message", "recall", "scope",
    "send", "setdata"
};

static ERRORINFO: phf::Set<&'static str> = phf_set! {
    "addaction", "addnavigationaction", "callstack", "collectible",
    "controlname", "create", "customdimensions", "dataclassification",
    "detailedmessage", "errortype", "fieldno", "message", "pageno", "recordid",
    "systemid", "tableid", "title", "verbosity"
};

static RECORDID: phf::Set<&'static str> = phf_set! {
    "getrecord", "tableno"
};

static MODULEINFO: phf::Set<&'static str> = phf_set! {
    "appversion", "dataversion", "dependencies", "id", "name", "packageid",
    "publisher"
};

static DATATRANSFER: phf::Set<&'static str> = phf_set! {
    "addconstantvalue", "adddestinationfilter", "addfieldvalue", "addjoin",
    "addsourcefilter", "copyfields", "copyrows", "settables", "updateauditfields"
};

static SESSIONSETTINGS: phf::Set<&'static str> = phf_set! {
    "company", "init", "languageid", "localeid", "profileappid", "profileid",
    "profilesystemscope", "requestsessionupdate", "timezone"
};

static FILTERPAGEBUILDER: phf::Set<&'static str> = phf_set! {
    "addfield", "addfieldno", "addrecord", "addrecordref", "addtable", "count",
    "getview", "name", "pagecaption", "runmodal", "setview"
};

static FILE: phf::Set<&'static str> = phf_set! {
    "close", "copy", "create", "createinstream", "createoutstream",
    "createtempfile", "download", "downloadfromstream", "erase", "exists",
    "getstamp", "ispathtemporary", "len", "name", "open", "pos", "read",
    "rename", "seek", "setstamp", "textmode", "trunc", "upload",
    "uploadintostream", "view", "viewfromstream", "write", "writemode"
};

static FILEUPLOAD: phf::Set<&'static str> = phf_set! {
    "createinstream", "filename"
};

static NUMBERSEQUENCE: phf::Set<&'static str> = phf_set! {
    "current", "delete", "exists", "insert", "next", "range", "restart"
};

static VERSION: phf::Set<&'static str> = phf_set! {
    "build", "create", "major", "minor", "revision", "totext"
};

static DIALOG: phf::Set<&'static str> = phf_set! {
    "close", "confirm", "error", "hidesubsequentdialogs", "loginternalerror",
    "message", "open", "strmenu", "update"
};

static PAGE_INSTANCE: phf::Set<&'static str> = phf_set! {
    "activate", "cancelbackgroundtask", "caption", "close", "editable",
    "enqueuebackgroundtask", "getbackgroundparameters", "getrecord",
    "lookupmode", "objectid", "promptmode", "run", "runmodal", "saverecord",
    "setbackgroundtaskresult", "setrecord", "setselectionfilter",
    "settableview", "update"
};

static REPORT_INSTANCE: phf::Set<&'static str> = phf_set! {
    "break", "createtotals", "defaultlayout", "excellayout", "execute",
    "formatregion", "isreadonly", "language", "newpage", "newpageperrecord",
    "objectid", "pageno", "papersource", "preview", "print",
    "printonlyifdetail", "quit", "rdlclayout", "run", "runmodal",
    "runrequestpage", "saveas", "saveasexcel", "saveashtml", "saveaspdf",
    "saveasword", "saveasxml", "settableview", "showoutput", "skip",
    "targetformat", "totalscausedby", "userequestpage",
    "validateandpreparelayout", "wordlayout", "wordxmlpart"
};

/// INSTANCE methods of the `Query` data type, per MS Learn's "Query data type"
/// reference (retrieved 2026-09-13). Reached only via a variable's DECLARED
/// object type (`V: Query "Name"`) — `object_instance_framework_kind` maps
/// `ObjectKind::Query` here; there is no `CurrQuery` receiver-name singleton.
///
/// The page also lists five STATIC overloads (`SaveAsCsv`/`SaveAsJson`/
/// `SaveAsXml` on the `Query` TYPE). Those are a different receiver surface and
/// are deliberately NOT modelled — no measured population, and the doctrine here
/// is to measure a population before building for it.
///
/// None of these dispatch into user code (they execute a platform query), so
/// none belongs in `ENTRY_DISPATCH_BUILTIN_IDS` — see that const's doc, which
/// is where the reasoning for excluding Query from the ENTRY-DISPATCH list was
/// mistaken for a reason to give Query no catalog at all.
static QUERY_INSTANCE: phf::Set<&'static str> = phf_set! {
    "close", "columncaption", "columnname", "columnno", "getfilter",
    "getfilters", "open", "read", "saveascsv", "saveasjson", "saveasxml",
    "securityfiltering", "setfilter", "setrange", "topnumberofrows"
};

static SESSION: phf::Set<&'static str> = phf_set! {
    "applicationarea", "applicationidentifier", "bindsubscription",
    "currentclienttype", "currentexecutionmode", "defaultclienttype",
    "enableverbosetelemetry", "getcurrentmoduleexecutioncontext",
    "getexecutioncontext", "getmoduleexecutioncontext", "issessionactive",
    "logauditmessage", "logmessage", "logsecurityaudit", "sendtracetag",
    "setdocumentservicetoken", "startsession", "stopsession",
    "unbindsubscription"
};

static NAVAPP: phf::Set<&'static str> = phf_set! {
    "deletearchivedata", "getarchiverecordref", "getarchiveversion",
    "getcallercallstackmoduleinfos", "getcallermoduleinfo",
    "getcurrentmoduleinfo", "getmoduleinfo", "getresource",
    "getresourceasjson", "getresourceastext", "isentitled", "isinstalling",
    "isunlicensed", "listresources", "loadpackagedata", "restorearchivedata"
};

static DATABASE: phf::Set<&'static str> = phf_set! {
    "alterkey", "changeuserpassword", "checklicensefile", "commit",
    "companyname", "copycompany", "currenttransactiontype",
    "datafileinformation", "exportdata", "getdefaulttableconnection",
    "hastableconnection", "importdata", "isinwritetransaction",
    "lastusedrowversion", "locktimeout", "locktimeoutduration",
    "minimumactiverowversion", "registertableconnection", "sid",
    "selectlatestversion", "serialnumber", "serviceinstanceid", "sessionid",
    "setdefaulttableconnection", "setuserpassword", "tenantid",
    "unregistertableconnection", "userid", "usersecurityid"
};

static ISOLATED_STORAGE: phf::Set<&'static str> = phf_set! {
    "contains", "delete", "get", "set", "setencrypted"
};

static TASKSCHEDULER: phf::Set<&'static str> = phf_set! {
    "cancreatetask", "canceltask", "createtask", "settaskready", "taskexists"
};

static SYSTEM: phf::Set<&'static str> = phf_set! {
    "abs", "applicationpath", "arraylen", "calcdate", "canloadtype",
    "captionclasstranslate", "clear", "clearall", "clearcollectederrors",
    "clearlasterror", "closingdate", "codecoverageinclude", "codecoverageload",
    "codecoveragelog", "codecoveragerefresh", "compressarray", "copyarray",
    "copystream", "createdatetime", "createencryptionkey", "createguid",
    "currentdatetime", "dmy2date", "dt2date", "dt2time", "dwy2date",
    "dati2variant", "date2dmy", "date2dwy", "decrypt", "deleteencryptionkey",
    "encrypt", "encryptionenabled", "encryptionkeyexists", "evaluate",
    "exportencryptionkey", "exportobjects", "format", "getcollectederrors",
    "getdocumenturl", "getdotnettype", "getlasterrorcallstack",
    "getlasterrorcode", "getlasterrorobject", "getlasterrortext", "geturl",
    "globallanguage", "guiallowed", "hascollectederrors", "hyperlink",
    "importencryptionkey", "importobjects", "importstreamwithurlaccess",
    "iscollectingerrors", "isnull", "isnullguid", "isservicetier",
    "normaldate", "power", "random", "randomize", "round", "rounddatetime",
    "sleep", "temporarypath", "time", "today", "variant2date", "variant2time",
    "windowslanguage", "workdate"
};

static COMPANY_PROPERTY: phf::Set<&'static str> = phf_set! {
    "displayname", "id", "urlname"
};

static SESSION_INFORMATION: phf::Set<&'static str> = phf_set! {
    "aitokensused", "callstack", "sqlrowsread", "sqlstatementsexecuted"
};

/// Enum VALUE-instance surface (Task 4, receiver-closure-and-arg-increments
/// plan — the SPLIT-catalog round-2 closer). Callable on an enum VALUE: a
/// declared `Enum "X"`-typed var/field, or an enum-value-literal chain
/// (`X::Y`) — `ReceiverType::EnumType`.
///
/// Provenance: MS Learn `enum-data-type` (fetched 2026-07-04), "Instance
/// methods" table — `AsInteger()` ("Get the enum value as an integer
/// value"), `Names()` ("Gets the value names"), `Ordinals()` ("Gets the
/// ordinal numbers/ID's for the values"). `FromInteger` is DELIBERATELY
/// ABSENT here — MS Learn lists it under "Static methods" only (see
/// `ENUM_TYPE_STATIC` below); converting FROM an integer produces a NEW enum
/// value and needs no existing value to invoke it against.
static ENUM_VALUE: phf::Set<&'static str> = phf_set! {
    "asinteger", "names", "ordinals"
};

/// Enum TYPE-static surface (Task 4). Callable on the enum TYPE reference
/// itself: `Enum::"Type"` (`ReceiverType::EnumTypeStatic`, an
/// `ExprKind::QualifiedEnum` whose `enum_type` is the literal `Enum` keyword)
/// or a bare (quoted or not) enum-type-name receiver that passed the
/// programmatic collision rule.
///
/// Provenance: MS Learn `enum-data-type` (fetched 2026-07-04), "Static
/// methods" table — `FromInteger(Integer)` ("Returns an enum with the
/// integer value"; its own doc page's syntax example is literally
/// `Enum::YesNo.FromInteger(10)`, the type-reference-receiver shape). `Names`
/// and `Ordinals` are ALSO real on this surface — confirmed by production AL
/// (`Enum::"CDO Module Type".Ordinals()`, real CDO sites, `Codeunit 6175279`
/// line 28 / `Codeunit 6175317` line 26) even though MS Learn's own
/// "instance methods" categorization would suggest otherwise: AL resolves
/// these purely by STATIC TYPE (no real object-instance dispatch), so the
/// enum-enumeration pair (`Names`/`Ordinals`, always documented together,
/// same "gets ... for/of the values" wording) is callable via EITHER
/// receiver shape identically — only `AsInteger`/`FromInteger` are
/// receiver-shape-specific (one needs an existing value to convert, the
/// other produces one). `AsInteger` is DELIBERATELY ABSENT here (round-2
/// closer, BINDING: "AsInteger is VALUE-surface... not TYPE-surface") — a
/// bare TYPE reference carries no specific value to convert, and no CDO
/// grounding contradicts excluding it.
static ENUM_TYPE_STATIC: phf::Set<&'static str> = phf_set! {
    "frominteger", "names", "ordinals"
};

// ---------------------------------------------------------------------------
// Private framework lookup
// ---------------------------------------------------------------------------

/// Return `true` if `method_lc` is a known member method for `fk`.
///
/// `ControlAddIn` is NOT a `FrameworkKind` variant (receiver-closure plan,
/// Task 1) — it moved to a dedicated `ReceiverType::ControlAddIn { name_lc,
/// surface }`, since (unlike every kind here) its member surface must be
/// gated on the SPECIFIC addin's declared procedures, not a single uniform
/// catalog. See `receiver::resolve_control_addin_receiver` and
/// `resolver::resolve_member_with_args`'s `ReceiverType::ControlAddIn` arm.
fn framework_lookup(fk: &FrameworkKind, method_lc: &str) -> bool {
    match fk {
        FrameworkKind::JsonObject => JSONOBJECT.contains(method_lc),
        FrameworkKind::JsonToken => JSONTOKEN.contains(method_lc),
        FrameworkKind::JsonArray => JSONARRAY.contains(method_lc),
        FrameworkKind::JsonValue => JSONVALUE.contains(method_lc),
        FrameworkKind::HttpClient => HTTPCLIENT.contains(method_lc),
        FrameworkKind::HttpRequestMessage => HTTPREQUEST.contains(method_lc),
        FrameworkKind::HttpResponseMessage => HTTPRESPONSE.contains(method_lc),
        FrameworkKind::HttpContent => HTTPCONTENT.contains(method_lc),
        FrameworkKind::HttpHeaders => HTTPHEADERS.contains(method_lc),
        FrameworkKind::InStream => INSTREAM.contains(method_lc),
        FrameworkKind::OutStream => OUTSTREAM.contains(method_lc),
        FrameworkKind::TextBuilder => TEXTBUILDER.contains(method_lc),
        FrameworkKind::Text => TEXT.contains(method_lc),
        FrameworkKind::BigText => BIGTEXT.contains(method_lc),
        FrameworkKind::SecretText => SECRETTEXT.contains(method_lc),
        FrameworkKind::List => LIST.contains(method_lc),
        FrameworkKind::Dictionary => DICTIONARY.contains(method_lc),
        FrameworkKind::Xml => XML.contains(method_lc),
        FrameworkKind::Date => DATE.contains(method_lc),
        FrameworkKind::DateTime => DATETIME.contains(method_lc),
        FrameworkKind::Time => TIME.contains(method_lc),
        FrameworkKind::Duration => DURATION.contains(method_lc),
        FrameworkKind::Guid => GUID.contains(method_lc),
        FrameworkKind::Blob => BLOB.contains(method_lc),
        FrameworkKind::Media => MEDIA.contains(method_lc),
        FrameworkKind::Notification => NOTIFICATION.contains(method_lc),
        FrameworkKind::ErrorInfo => ERRORINFO.contains(method_lc),
        FrameworkKind::RecordId => RECORDID.contains(method_lc),
        FrameworkKind::ModuleInfo => MODULEINFO.contains(method_lc),
        FrameworkKind::DataTransfer => DATATRANSFER.contains(method_lc),
        FrameworkKind::SessionSettings => SESSIONSETTINGS.contains(method_lc),
        FrameworkKind::FilterPageBuilder => FILTERPAGEBUILDER.contains(method_lc),
        FrameworkKind::File => FILE.contains(method_lc),
        FrameworkKind::FileUpload => FILEUPLOAD.contains(method_lc),
        FrameworkKind::NumberSequence => NUMBERSEQUENCE.contains(method_lc),
        FrameworkKind::Version => VERSION.contains(method_lc),
        FrameworkKind::Dialog => DIALOG.contains(method_lc),
        FrameworkKind::PageInstance => PAGE_INSTANCE.contains(method_lc),
        FrameworkKind::ReportInstance => REPORT_INSTANCE.contains(method_lc),
        FrameworkKind::QueryInstance => QUERY_INSTANCE.contains(method_lc),
        FrameworkKind::Session => SESSION.contains(method_lc),
        FrameworkKind::NavApp => NAVAPP.contains(method_lc),
        FrameworkKind::Database => DATABASE.contains(method_lc),
        FrameworkKind::IsolatedStorage => ISOLATED_STORAGE.contains(method_lc),
        FrameworkKind::TaskScheduler => TASKSCHEDULER.contains(method_lc),
        FrameworkKind::System => SYSTEM.contains(method_lc),
        FrameworkKind::CompanyProperty => COMPANY_PROPERTY.contains(method_lc),
        FrameworkKind::SessionInformation => SESSION_INFORMATION.contains(method_lc),
        FrameworkKind::Enum => ENUM_VALUE.contains(method_lc),
        FrameworkKind::EnumTypeStatic => ENUM_TYPE_STATIC.contains(method_lc),
        // Unknown/programmatic type — not in catalog.
        FrameworkKind::Other(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return `true` if `method_lc` (already lowercased) is a known builtin method
/// for the given `kind`.
pub fn member_builtin(kind: MemberCatalogKind<'_>, method_lc: &str) -> bool {
    match kind {
        MemberCatalogKind::RecordRef => RECORDREF.contains(method_lc),
        MemberCatalogKind::FieldRef => FIELDREF.contains(method_lc),
        MemberCatalogKind::KeyRef => KEYREF.contains(method_lc),
        MemberCatalogKind::Record => RECORD.contains(method_lc),
        MemberCatalogKind::Framework(fk) => framework_lookup(fk, method_lc),
    }
}

/// Return `Some(BuiltinId)` if `method_lc` (already lowercased) is a known
/// builtin method for the given `kind`, otherwise `None`.
///
/// The `BuiltinId` string is `"{Prefix}::{method_lc}"` where `Prefix` is:
/// - `"RecordRef"` / `"FieldRef"` / `"KeyRef"` / `"Record"` for those kinds.
/// - `format!("{fk:?}")` for `Framework(fk)` (e.g. `"JsonObject"`, `"HttpClient"`).
pub fn member_builtin_id(kind: MemberCatalogKind<'_>, method_lc: &str) -> Option<BuiltinId> {
    if !member_builtin(kind, method_lc) {
        return None;
    }
    let prefix = match kind {
        MemberCatalogKind::RecordRef => "RecordRef".to_string(),
        MemberCatalogKind::FieldRef => "FieldRef".to_string(),
        MemberCatalogKind::KeyRef => "KeyRef".to_string(),
        MemberCatalogKind::Record => "Record".to_string(),
        MemberCatalogKind::Framework(fk) => format!("{fk:?}"),
    };
    Some(BuiltinId(format!("{prefix}::{method_lc}")))
}

// ---------------------------------------------------------------------------
// T0.3: entry-dispatching builtin catalog members (diagnostic-only)
// ---------------------------------------------------------------------------

/// [`BuiltinId`] text (`"{Prefix}::{method_lc}"`, see [`member_builtin_id`]) of
/// every catalog member that DISPATCHES INTO USER CODE when its target object
/// is statically known, rather than being an ordinary platform intrinsic.
///
/// `Run`/`RunModal` on the Page/Report instance catalogs (`PAGE_INSTANCE`
/// above, `member_catalog.rs:307`; `REPORT_INSTANCE`, `:316`) open the named
/// page/report — a real entry-trigger dispatch into caller-named user code,
/// not a leaf platform call. T0.3 found TWO classifier gaps that landed this
/// shape as an ordinary `Evidence::Catalog` `Builtin` route instead of an
/// entry-trigger `Run` edge into the target object; T1.3 (deep-review-
/// remediation plan) closed BOTH:
/// - `extract::classify_call`'s `ObjectRun` check
///   (`src/program/resolve/extract.rs`) now also accepts `RunModal` (not just
///   `Run`) for a Page/Report KEYWORD receiver (`Page`/`Report` used as a
///   pseudo-namespace) — classifying it as `CalleeShape::ObjectRun` instead of
///   falling through to `Member`.
/// - `resolver::resolve_member_with_args`'s `Object{kind, name_lc}` arm now
///   dispatches a declared Page/Report-typed VARIABLE receiver's
///   `Run`/`RunModal` call (`MyPage.RunModal()`) to the entry trigger too,
///   via the same `resolver::dispatch_entry_trigger` machinery
///   `resolve_object_run`'s keyword-receiver form uses — not just
///   `Codeunit.Run`.
///
/// Because of this fix, `is_entry_dispatch_builtin` should now be
/// UNREACHABLE for `Run`/`RunModal` sites in practice — every such call
/// classifies as `ObjectRun` or hits the entry-trigger special case before
/// ever reaching the instance-builtin catalog fallback that produces a
/// `Builtin` route. The const and audit stay in place as a live regression
/// backstop (see the T0.3/T1.3 CDO ratchet,
/// `cdo_builtin_dispatch_audit_flagged_count_is_pinned`, pinned at 0) — a
/// future classifier change that reopens either gap would re-populate
/// `audit.flagged` and fail that pin.
///
/// This const is consulted ONLY by the T0.3 builtin-dispatch audit
/// (`program::resolve::full`'s `builtin_dispatch_finding`) — NEVER by
/// resolution/classification itself. Diagnostic-only, does not change any
/// route/edge/histogram.
///
/// # Inclusion / exclusion (scouted against every catalog in this file)
///
/// - **Included:** `PageInstance::run`, `PageInstance::runmodal`,
///   `ReportInstance::run`, `ReportInstance::runmodal` — MS Learn documents
///   both as opening the page/report named by the call
///   (`page-run-method`/`page-runmodal-method`,
///   `report-run-method`/`report-runmodal-method`): a genuine entry dispatch
///   into the target object's own trigger chain (`OnOpenPage`/`OnInit` for
///   Page, `OnInitReport`/`OnPreReport` for Report).
/// - **Codeunit excluded:** `Codeunit.Run`/a declared Codeunit-typed
///   variable's `.Run()` can NEVER land in `builtin` — the keyword-receiver
///   form is caught by `extract::classify_call`'s `ObjectRun` check (method
///   `"run"` already matches there), and the variable-receiver form is caught
///   by `resolve_member_with_args`'s OWN `ObjectKind::Codeunit && method_lc ==
///   "run"` special case (`resolver.rs:2433`), which dispatches to the
///   `OnRun` entry trigger directly. `Codeunit` has no `RunModal` member at
///   all (MS Learn: only Page/Report document a `RunModal` overload) and
///   `object_instance_framework_kind` (`resolver.rs:2137`) returns `None` for
///   `Codeunit` — there is no instance-builtin catalog for it to fall into.
/// - **XmlPort/Query excluded from THIS list, for a reason about dispatch —
///   not about member existence.** Neither has a `Run`/`RunModal`-shaped member
///   that dispatches into user code per MS Learn: `XmlPort.Import`/`Export`
///   stream data through a declared XmlPort's OWN ports (not a fan-out into a
///   NAMED target), and `Query.Open`/`Read`/`Close` execute a platform query,
///   never a callee's code. So neither belongs in this ENTRY-DISPATCH list.
///
///   **That is NOT a reason to give them no catalog, and treating it as one was
///   a real defect** (2026-09-13): `object_instance_framework_kind` returned
///   `None` for `ObjectKind::Query`, so `V: Query "X"; V.Open()` had no catalog
///   to resolve against and became `Unknown(MemberNotFound)` — 3 such sites on
///   CDO, the entire residual of the north-star metric. `QUERY_INSTANCE` above
///   now supplies the members; they stay out of THIS list, which is correct.
///   This is the same conflation the Page/Report catalog fixed once already —
///   see `is_metadata_sensitive_instance_method`'s history note.
///   **`XmlPort` still has no catalog** and will fail the same way the moment a
///   workspace declares an XmlPort variable and calls `Import`/`Export` on it.
///   Left unbuilt deliberately: zero measured population today, and this repo's
///   standing rule is to measure the population before building for it.
pub const ENTRY_DISPATCH_BUILTIN_IDS: &[&str] = &[
    "PageInstance::run",
    "PageInstance::runmodal",
    "ReportInstance::run",
    "ReportInstance::runmodal",
];

/// `true` when `id`'s text is one of [`ENTRY_DISPATCH_BUILTIN_IDS`] — a
/// `Builtin` route the T0.3 audit must justify (flag or mark indeterminate)
/// rather than silently accept. Diagnostic-only (see the const's doc).
#[must_use]
pub fn is_entry_dispatch_builtin(id: &BuiltinId) -> bool {
    ENTRY_DISPATCH_BUILTIN_IDS.contains(&id.0.as_str())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `QUERY_INSTANCE` membership, per MS Learn's Query data type reference.
    /// The wiring that routes a `Query` object here is pinned separately in
    /// `resolver.rs` — a correct set nothing routes to is the failure mode that
    /// actually occurred.
    #[test]
    fn query_instance_members_resolve() {
        let kind = FrameworkKind::QueryInstance;
        // The three CDO actually calls.
        for m in ["open", "read", "close"] {
            assert!(member_builtin(MemberCatalogKind::Framework(&kind), m));
        }
        // A representative spread of the rest of the documented surface.
        for m in [
            "setrange",
            "setfilter",
            "topnumberofrows",
            "getfilters",
            "saveasxml",
        ] {
            assert!(member_builtin(MemberCatalogKind::Framework(&kind), m));
        }
        // Not a Query member: `Find` is Record vocabulary, and catalogs must not
        // accept a method merely because some other AL type has it.
        assert!(!member_builtin(MemberCatalogKind::Framework(&kind), "find"));
        assert!(!member_builtin(
            MemberCatalogKind::Framework(&kind),
            "notamethod"
        ));
    }

    #[test]
    fn json_object_members_resolve() {
        let kind = FrameworkKind::JsonObject;
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "add"));
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "get"));
        assert!(member_builtin(
            MemberCatalogKind::Framework(&kind),
            "contains"
        ));
        assert!(!member_builtin(
            MemberCatalogKind::Framework(&kind),
            "notamethod"
        ));
    }

    #[test]
    fn fieldref_members_resolve() {
        assert!(member_builtin(MemberCatalogKind::FieldRef, "value"));
        assert!(member_builtin(MemberCatalogKind::FieldRef, "validate"));
        assert!(member_builtin(MemberCatalogKind::FieldRef, "name"));
        assert!(!member_builtin(MemberCatalogKind::FieldRef, "notamethod"));
    }

    #[test]
    fn httpclient_members_resolve() {
        let kind = FrameworkKind::HttpClient;
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "get"));
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "post"));
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "put"));
        assert!(!member_builtin(
            MemberCatalogKind::Framework(&kind),
            "fetch"
        ));
    }

    #[test]
    fn recordref_members_resolve() {
        assert!(member_builtin(MemberCatalogKind::RecordRef, "field"));
        assert!(member_builtin(MemberCatalogKind::RecordRef, "open"));
        assert!(member_builtin(MemberCatalogKind::RecordRef, "close"));
        assert!(!member_builtin(MemberCatalogKind::RecordRef, "notamethod"));
    }

    #[test]
    fn record_members_resolve() {
        assert!(member_builtin(MemberCatalogKind::Record, "find"));
        assert!(member_builtin(MemberCatalogKind::Record, "setrange"));
        assert!(member_builtin(MemberCatalogKind::Record, "insert"));
        assert!(!member_builtin(
            MemberCatalogKind::Record,
            "calculatediscount"
        ));
    }

    #[test]
    fn keyref_members_resolve() {
        assert!(member_builtin(MemberCatalogKind::KeyRef, "fieldcount"));
        assert!(member_builtin(MemberCatalogKind::KeyRef, "fieldindex"));
        assert!(!member_builtin(MemberCatalogKind::KeyRef, "notamethod"));
    }

    // `controladdin_any_method_is_builtin` (the pre-Task-1 "every ControlAddIn
    // method is unconditionally builtin" test) was REMOVED here, not flipped —
    // `FrameworkKind::ControlAddIn` no longer exists (receiver-closure plan,
    // Task 1: the addin's member surface is now gated per-instance, not a
    // single uniform catalog entry). Its closed-if-known replacement lives in
    // `resolver.rs`'s `ReceiverType::ControlAddIn` dispatch tests
    // (`resolve_member_controladdin_*`), and the TRUE unconditional-accept
    // case that survives (`ControlAddInSurface::TruePlatform`) is covered
    // there too (`resolve_member_controladdin_true_platform_any_method_is_catalog`).

    #[test]
    fn other_kind_returns_false() {
        let kind = FrameworkKind::Other("unknowntype".to_string());
        assert!(!member_builtin(
            MemberCatalogKind::Framework(&kind),
            "somemethod"
        ));
    }

    #[test]
    fn member_builtin_id_format() {
        let kind = FrameworkKind::JsonObject;
        let id = member_builtin_id(MemberCatalogKind::Framework(&kind), "add").unwrap();
        assert_eq!(id.0, "JsonObject::add");

        let id2 = member_builtin_id(MemberCatalogKind::FieldRef, "value").unwrap();
        assert_eq!(id2.0, "FieldRef::value");

        let id3 = member_builtin_id(MemberCatalogKind::RecordRef, "open").unwrap();
        assert_eq!(id3.0, "RecordRef::open");

        let id4 = member_builtin_id(MemberCatalogKind::Record, "insert").unwrap();
        assert_eq!(id4.0, "Record::insert");

        assert!(member_builtin_id(MemberCatalogKind::Framework(&kind), "notamethod").is_none());
    }

    /// beyond-1B.3b Task 1 review-fix (Finding 2): pins the catalog's ACTUAL
    /// structural guarantee — membership is an EXACT-STRING lookup in a
    /// RECEIVER-KIND-SCOPED `phf::Set` (selected by `kind`), with no hash/
    /// fingerprint digest stored or compared, so a near-miss name (not a real
    /// catalog member for that receiver kind) and a cross-kind miss (a real
    /// member of a DIFFERENT receiver kind's set) are both fail-closed
    /// REJECTED (`None`), never classified `builtin` by coincidence.
    ///
    /// (A prior revision of this test exercised a `member_builtin_id_checked`
    /// wrapper that re-derived the same query and compared its own output to
    /// itself — an unreachable guard, since `BuiltinId`'s suffix is built
    /// directly from `method_lc` and `member_builtin` already scopes
    /// membership by receiver kind before the wrapper's own check could run.
    /// The wrapper added no behavior beyond `member_builtin_id`/
    /// `member_builtin` themselves, so it was removed; THIS test now asserts
    /// the real fail-closed contract directly against the phf-backed
    /// functions that resolver.rs actually calls.)
    #[test]
    fn member_builtin_id_is_name_exact_and_rejects_near_miss() {
        let id = member_builtin_id(MemberCatalogKind::Record, "setrange").unwrap();
        assert_eq!(id.0, "Record::setrange");

        // Near-miss: not a real catalog member for this receiver kind.
        assert!(member_builtin_id(MemberCatalogKind::Record, "setrangex").is_none());
        assert!(!member_builtin(MemberCatalogKind::Record, "setrangex"));
        // Cross-kind miss: "strlen" is a Text/global method, not a Record method.
        assert!(member_builtin_id(MemberCatalogKind::Record, "strlen").is_none());
        assert!(!member_builtin(MemberCatalogKind::Record, "strlen"));
    }

    #[test]
    fn coverage_guard_min_membership() {
        // Guard against accidentally empty/truncated sets after regen.
        // Key sets sizes from JSON source:
        let total = JSONOBJECT.len()
            + RECORDREF.len()
            + RECORD.len()
            + HTTPCLIENT.len()
            + XML.len()
            + FIELDREF.len();
        assert!(
            total >= 200,
            "catalog must have ≥200 entries across key sets; got {total}"
        );
    }

    // -- Enum catalog split (Task 4, receiver-closure-and-arg-increments plan) --

    /// VALUE-instance surface: AsInteger/Names/Ordinals resolve; FromInteger
    /// is ABSENT (MS Learn: static-only — see `ENUM_VALUE`'s doc).
    #[test]
    fn enum_value_surface_has_asinteger_names_ordinals_not_frominteger() {
        let kind = FrameworkKind::Enum;
        assert!(member_builtin(
            MemberCatalogKind::Framework(&kind),
            "asinteger"
        ));
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "names"));
        assert!(member_builtin(
            MemberCatalogKind::Framework(&kind),
            "ordinals"
        ));
        assert!(!member_builtin(
            MemberCatalogKind::Framework(&kind),
            "frominteger"
        ));
    }

    /// TYPE-static surface: FromInteger/Names/Ordinals resolve; AsInteger is
    /// ABSENT (round-2 closer, BINDING — see `ENUM_TYPE_STATIC`'s doc).
    #[test]
    fn enum_type_static_surface_has_frominteger_names_ordinals_not_asinteger() {
        let kind = FrameworkKind::EnumTypeStatic;
        assert!(member_builtin(
            MemberCatalogKind::Framework(&kind),
            "frominteger"
        ));
        assert!(member_builtin(MemberCatalogKind::Framework(&kind), "names"));
        assert!(member_builtin(
            MemberCatalogKind::Framework(&kind),
            "ordinals"
        ));
        assert!(!member_builtin(
            MemberCatalogKind::Framework(&kind),
            "asinteger"
        ));
    }

    #[test]
    fn enum_type_static_builtin_id_format() {
        let kind = FrameworkKind::EnumTypeStatic;
        let id = member_builtin_id(MemberCatalogKind::Framework(&kind), "frominteger").unwrap();
        assert_eq!(id.0, "EnumTypeStatic::frominteger");
    }
}
