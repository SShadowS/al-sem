//! The last overload ambiguities in Continia DO's dependency code, each a call
//! the compiler binds one way (alc 18.0.43 compiles every fixture below with
//! no AL0196) and the engine left `ambiguousResolved`.
//!
//! - an Integer literal guard that ignored a candidate another position rules
//!   out (System Application's `AddBlobPutContentHeaders(…, BlobType::
//!   AppendBlob, 0, …)`);
//! - a Variant argument next to typed ones (`WordTemplates.Merge(DataVariant,
//!   SplitDocuments, …)`);
//! - a call result typed by overloads that all return the same type
//!   (`CreateFile(…, Helper.GetContentLength(InStream), …)`);
//! - an interface method's result (`SetFunctionAsToolChoice(Function.GetName())`);
//! - a DotNet stream against an AL object (`SaveZipArchive(MemoryStream)`);
//! - `CreateGuid()` as a Guid (`Convert.ToBase64(CreateGuid())`);
//! - a Text-family argument over `SecretText` (`AddHeader(Label, Label)`,
//!   `UrlEncode(Label)`, `VerifyData(Text, Text, …)`).

use std::process::Command;

fn stats(name: &str, source: &str, on_prem: bool) -> serde_json::Value {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join(name);
    std::fs::create_dir_all(ws.join("src")).unwrap();
    let target = if on_prem { r#","target":"OnPrem""# } else { "" };
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"42424242-0000-4000-8000-000000000042","name":"{name}","publisher":"t","version":"1.0.0.0"{target}}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), source).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--program-call-graph-stats")
        .arg(&ws)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    v["primaryScoped"].clone()
}

fn assert_resolved(name: &str, source: &str, on_prem: bool, resolved: u64) {
    let s = stats(name, source, on_prem);
    assert_eq!(s["ambiguousResolved"], 0, "{name}: {s}");
    assert_eq!(s["resolvedSource"], resolved, "{name}: {s}");
}

#[test]
fn an_integer_literal_does_not_block_a_pick_another_position_proves() {
    assert_resolved(
        "blob",
        r#"enum 50110 "BT" { value(0; AppendBlob) { } }
codeunit 50110 P { }
codeunit 50111 H
{
    local procedure Add(var C: HttpContent; P: Codeunit P; var S: InStream; B: Enum "BT"; T: Text) begin end;
    local procedure Add(var C: HttpContent; P: Codeunit P; S: Text; B: Enum "BT"; T: Text) begin end;
    local procedure Add(var C: HttpContent; P: Codeunit P; B: Enum "BT"; L: BigInteger; T: Text) begin end;
    procedure Go(var C: HttpContent; P: Codeunit P; BlobType: Enum "BT"; T: Text)
    begin
        Add(C, P, BlobType::AppendBlob, 0, T);
    end;
}
"#,
        false,
        1,
    );
}

#[test]
fn a_variant_argument_next_to_typed_ones_binds_by_elimination() {
    assert_resolved(
        "merge",
        r#"enum 50100 "SF" { value(0; Docx) { } }
enum 50101 "CB" { value(0; A) { } }
codeunit 50100 Lib
{
    procedure Merge(Data: Dictionary of [Text, Text]; SaveFormat: Enum "SF"; EditDoc: Boolean; C: Enum "CB") begin end;
    procedure Merge(RecordVariant: Variant; SplitDocument: Boolean; SaveFormat: Enum "SF"; EditDoc: Boolean) begin end;
    procedure Put(D: Dictionary of [Text, Text]; B: Boolean) begin end;
    procedure Put(T: Text; N: Integer) begin end;
}
codeunit 50102 X
{
    procedure Go()
    var
        L: Codeunit Lib;
        D: Variant;
        B: Boolean;
    begin
        L.Merge(D, B, Enum::"SF"::Docx, true);
        // A Variant converts into the Dictionary; the Boolean rules out
        // `(Text; Integer)`.
        L.Put(D, B);
    end;
}
"#,
        false,
        2,
    );
}

#[test]
fn overloads_that_agree_on_the_return_type_type_the_call() {
    // Two edges: `H.Len(S)` itself and the `CreateFile` it feeds.
    assert_resolved(
        "cfile",
        r#"codeunit 50120 O { }
codeunit 50121 R { }
codeunit 50122 HC
{
    procedure Len(var S: InStream): Integer begin end;
    procedure Len(S: Text): Integer begin end;
}
codeunit 50123 F
{
    procedure CreateFile(P: Text; S: InStream; O: Codeunit O): Codeunit R begin end;
    procedure CreateFile(P: Text; N: Integer; O: Codeunit O): Codeunit R begin end;
    procedure Go(P: Text; S: InStream; O: Codeunit O): Codeunit R
    var
        H: Codeunit HC;
    begin
        exit(CreateFile(P, H.Len(S), O));
    end;
}
"#,
        false,
        2,
    );
}

#[test]
fn an_interface_method_result_is_its_declared_type() {
    assert_resolved(
        "iface",
        r#"interface "IF" { procedure GetName(): Text; }
codeunit 50130 T
{
    procedure Choose(Function: Interface "IF") begin end;
    procedure Choose(FunctionName: Text) begin end;
    procedure Go(Function: Interface "IF")
    begin
        Choose(Function.GetName());
    end;
}
"#,
        false,
        1,
    );
}

#[test]
fn a_dotnet_value_never_binds_an_al_object_or_wrapper_type() {
    assert_resolved(
        "dotnet",
        r#"codeunit 50140 TB { }
codeunit 50141 DC
{
    procedure Save(O: OutStream) begin end;
    procedure Save(var B: Codeunit TB) begin end;
    procedure Find(N: DotNet XmlNode; var R: DotNet XmlNode) begin end;
    procedure Find(N: XmlNode; var R: XmlNode) begin end;
    procedure Go()
    var
        MemoryStream: DotNet MemoryStream;
        Doc: DotNet XmlDocument;
        R: DotNet XmlNode;
    begin
        Save(MemoryStream);
        // The DotNet member is untyped; the `var` DotNet node rules out
        // `(XmlNode; var XmlNode)` (Base Application's `XMLDOMMgt.FindNode(
        // DotNetDoc.DocumentElement, Path, XmlRootNode)`).
        Find(Doc.DocumentElement, R);
    end;
}
dotnet
{
    assembly(mscorlib) { type(System.IO.MemoryStream; MemoryStream) { } }
    assembly(System.Xml)
    {
        type(System.Xml.XmlNode; XmlNode) { }
        type(System.Xml.XmlDocument; XmlDocument) { }
    }
}
"#,
        true,
        2,
    );
}

#[test]
fn create_guid_is_a_guid_argument() {
    assert_resolved(
        "guid",
        r#"codeunit 50150 B
{
    procedure ToBase64(S: Text): Text begin end;
    procedure ToBase64(S: SecretText): SecretText begin end;
    procedure ToBase64(S: InStream): Text begin end;
    procedure Go(): Text
    begin
        exit(ToBase64(CreateGuid()));
    end;
}
"#,
        false,
        1,
    );
}

#[test]
fn a_text_family_argument_binds_text_over_secrettext() {
    assert_resolved(
        "secret",
        r#"codeunit 50160 C
{
    procedure F(V: Text): Integer begin end;
    procedure F(V: SecretText): Boolean begin end;
    procedure G(K: Text; V: Text): Integer begin end;
    procedure G(K: Text; V: SecretText): Boolean begin end;
    procedure V(A: Text; X: SecretText): Integer begin end;
    procedure V(A: SecretText; X: SecretText): Boolean begin end;
    procedure Go(A: Text; X: Text)
    var
        I: Integer;
        L: Label 'x', Locked = true;
        C: Code[20];
    begin
        I := F(L);
        I := F(C);
        I := G(L, L);
        I := V(A, X);
    end;
}
"#,
        false,
        4,
    );
}
