// gen-al-var-params (ENG-135): offline generator of the var-ness of every AL platform method's
// parameters, read from the AL compiler's own symbols, emitted as
// src/program/resolve/platform_var_params.rs.
//
// Each built-in class (`*ClassTypeSymbol` in Microsoft.Dynamics.Nav.CodeAnalysis.dll: System for the
// global methods, Dialog, Database, Table for records, InStream, JsonObject, ...) is read through its
// static `Instance`; the generic Dictionary and List through a compiled parameter of each (their
// members sit on the parameter type's BaseType). Every method overload's IParameterSymbol.IsVar is
// read. A method name maps to the argument positions that ANY overload of that name, on ANY class,
// takes as `var`: a call site may not know its receiver's type or overload, so the union is the
// sound answer to "may this call write argument k".
//
// Run manually from the repo root (not in CI or cargo):
//   dotnet run --project tools/gen-al-var-params/gen.csproj -- <alc compiler dir> src/program/resolve/platform_var_params.rs
// The compiler dir is alc's tools/net10.0/any (the one holding Microsoft.Dynamics.Nav.CodeAnalysis.dll).
using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using Microsoft.Dynamics.Nav.CodeAnalysis;
using Microsoft.Dynamics.Nav.CodeAnalysis.Syntax;

class Program
{
    static int Main(string[] args)
    {
        if (args.Length != 2)
        {
            Console.Error.WriteLine("usage: gen-al-var-params <alc compiler dir> <output .rs>");
            return 2;
        }
        var dir = args[0];
        AppDomain.CurrentDomain.AssemblyResolve += (_, e) =>
        {
            var p = Path.Combine(dir, new AssemblyName(e.Name).Name + ".dll");
            return File.Exists(p) ? Assembly.LoadFrom(p) : null;
        };
        return Run(dir, args[1]);
    }

    static IEnumerable<object> MembersOf(object typeSymbol)
    {
        var gm = typeSymbol.GetType().GetMethod("GetMembers",
            BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance, Type.EmptyTypes);
        return ((IEnumerable)gm.Invoke(typeSymbol, null)).Cast<object>();
    }

    // method name (lowercase) -> var positions, and the counts for the header.
    static readonly SortedDictionary<string, ulong> VarPositions = new(StringComparer.Ordinal);
    static readonly SortedSet<string> AllNames = new(StringComparer.Ordinal);
    static int overloads, classes;

    static void Read(IEnumerable<object> members)
    {
        classes++;
        foreach (var m in members.OfType<IMethodSymbol>())
        {
            overloads++;
            var name = m.Name.ToLowerInvariant();
            AllNames.Add(name);
            ulong mask = 0;
            foreach (var p in m.Parameters)
            {
                if (!p.IsVar) continue;
                if (p.Ordinal >= 64) throw new Exception($"{m.Name}: var parameter {p.Ordinal} past 63");
                mask |= 1UL << p.Ordinal;
            }
            VarPositions.TryGetValue(name, out var had);
            VarPositions[name] = had | mask;
        }
    }

    static int Run(string dir, string output)
    {
        var asm = typeof(Compilation).Assembly;
        Type[] types;
        try { types = asm.GetTypes(); }
        catch (ReflectionTypeLoadException e) { types = e.Types.Where(t => t != null).ToArray(); }
        // Abstract bases every class derives from, and the interface type's base: no members.
        var bases = new HashSet<string> { "Language", "GenericLanguage", "I" };
        var unread = new List<string>();
        foreach (var t in types.Where(t => t.Name.EndsWith("ClassTypeSymbol") && !t.IsNested).OrderBy(t => t.Name))
        {
            var cls = t.Name.Substring(0, t.Name.Length - "ClassTypeSymbol".Length);
            var inst = t.GetProperty("Instance", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic)
                ?.GetValue(null);
            if (inst != null) Read(MembersOf(inst));
            else if (!bases.Contains(cls)) unread.Add(cls);
        }
        // Generic classes, through a compiled parameter of each (no symbol packages needed).
        var generic = new (string cls, string decl)[]
        {
            ("Dictionary", "Dictionary of [Integer, Integer]"),
            ("List", "List of [Integer]"),
        };
        var src = "codeunit 50135 \"VarParams Probe\"\n{\n    procedure P(" +
                  string.Join("; ", generic.Select((g, i) => $"P{i}: {g.decl}")) + ")\n    begin\n    end;\n}\n";
        var tree = SyntaxTree.ParseObjectText(src);
        var comp = Compilation.Create("probe", "RC", new Version(1, 0, 0, 0), null, null, new[] { tree },
            null, null, null, null, false, default);
        var model = comp.GetSemanticModel(tree);
        var ps = tree.GetRoot(default).DescendantNodes().OfType<ParameterSyntax>().ToList();
        for (int i = 0; i < generic.Length; i++)
        {
            var ty = ((IParameterSymbol)model.GetDeclaredSymbol(ps[i], default)).ParameterType;
            var baseType = ty.GetType()
                .GetProperty("BaseType", BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                .GetValue(ty);
            Read(MembersOf(baseType));
            unread.Remove(generic[i].cls);
        }
        if (unread.Count > 0)
        {
            // A class whose methods are not read could hide a writer: refuse rather than emit
            // an unsound table.
            Console.Error.WriteLine("unread classes: " + string.Join(", ", unread));
            return 1;
        }

        var version = FileVersionInfo(Path.Combine(dir, "Microsoft.Dynamics.Nav.CodeAnalysis.dll"));
        var writers = VarPositions.Count(kv => kv.Value != 0);
        var sb = new StringBuilder();
        sb.Append("// @generated by tools/gen-al-var-params (ENG-135). Do not edit by hand: re-run the generator.\n");
        sb.Append($"// Source: Microsoft.Dynamics.Nav.CodeAnalysis.dll {version}; {classes} classes, {overloads} method\n");
        sb.Append($"// overloads, {AllNames.Count} method names, {writers} of them with a `var` parameter.\n\n");
        sb.Append("//! Every AL platform method name (lowercase) with the argument positions that some overload of\n");
        sb.Append("//! it, on some built-in class, takes as `var` (bit k = argument k, 0-based), read from the AL\n");
        sb.Append("//! compiler's own symbols. Sorted by name for a binary search.\n\n");
        sb.Append("pub(crate) static PLATFORM_VAR_PARAMS: &[(&str, u64)] = &[\n");
        foreach (var name in AllNames)
            sb.Append($"    ({Quote(name)}, 0x{VarPositions[name]:x}),\n");
        sb.Append("];\n");
        File.WriteAllText(output, sb.ToString());
        Console.Error.WriteLine($"{classes} classes, {overloads} overloads, {AllNames.Count} names, {writers} writers -> {output}");
        return 0;
    }

    static string Quote(string s) => "\"" + s.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\"";

    static string FileVersionInfo(string path) =>
        System.Diagnostics.FileVersionInfo.GetVersionInfo(path).FileVersion ?? "unknown";
}
