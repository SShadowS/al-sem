using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Resources;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;

// gen-al-builtins: Offline generator — extracts the complete AL platform global-builtin
// catalog from the AL compiler DLL's embedded ClassDocumentationResources, then emits:
//   src/program/resolve/global_builtins.rs  — a phf::phf_set! for bare-call reclassification
//   tools/gen-al-builtins/out/member_builtins.json — full Type→[methods] map for Task 3
//
// Run manually (not in CI/cargo build) from the repo root:
//   dotnet run --project tools/gen-al-builtins/gen.csproj
// or pass the AL bin\win32 dir as args[0].

class AlLoadContext : AssemblyLoadContext
{
    private readonly string _binDir;
    public AlLoadContext(string binDir) { _binDir = binDir; }
    protected override Assembly Load(AssemblyName name)
    {
        var candidate = Path.Combine(_binDir, name.Name + ".dll");
        return File.Exists(candidate) ? LoadFromAssemblyPath(candidate) : null;
    }
}

class Program
{
    static int Main(string[] args)
    {
        string binDir = args.Length > 0 ? args[0]
            : @"C:\Users\SShadowS\.vscode\extensions\ms-dynamics-smb.al-18.0.2293710\bin\win32";

        if (!Directory.Exists(binDir))
        {
            Console.Error.WriteLine($"ERROR: AL bin\\win32 dir not found: {binDir}");
            return 1;
        }

        string dllPath = Path.Combine(binDir, "Microsoft.Dynamics.Nav.CodeAnalysis.dll");
        if (!File.Exists(dllPath))
        {
            Console.Error.WriteLine($"ERROR: DLL not found: {dllPath}");
            return 1;
        }

        // Load via AlLoadContext so sibling DLLs resolve from the same bin\win32 folder.
        var ctx = new AlLoadContext(binDir);
        var asm = ctx.LoadFromAssemblyPath(dllPath);

        // Find the embedded resource with documentation keys.
        string[] resourceNames = asm.GetManifestResourceNames();
        string found = resourceNames.FirstOrDefault(r => r.EndsWith("ClassDocumentationResources.resources",
            StringComparison.OrdinalIgnoreCase));
        if (found == null)
        {
            Console.Error.WriteLine("ERROR: ClassDocumentationResources.resources not found in DLL.");
            Console.Error.WriteLine("Available resources:");
            foreach (var r in resourceNames) Console.Error.WriteLine($"  {r}");
            return 1;
        }

        // Read all keys from the resource.
        var typeMap = new SortedDictionary<string, SortedSet<string>>(StringComparer.Ordinal);
        using (var stream = asm.GetManifestResourceStream(found))
        using (var reader = new ResourceReader(stream))
        {
            foreach (DictionaryEntry entry in reader)
            {
                string key = entry.Key?.ToString() ?? "";

                // Skip OPTION_* (enum docs) — we only want Type_Method (2 segments).
                if (key.StartsWith("OPTION_", StringComparison.Ordinal))
                    continue;

                var parts = key.Split('_');
                // Exactly 2 segments → Type_Method (no param/return suffix).
                if (parts.Length != 2)
                    continue;

                string typeName = parts[0];
                string methodName = parts[1];

                if (!typeMap.ContainsKey(typeName))
                    typeMap[typeName] = new SortedSet<string>(StringComparer.Ordinal);
                typeMap[typeName].Add(methodName);
            }
        }

        int typeCount = typeMap.Count;
        // Union of all method names (deduped, lowercased) for the global phf_set.
        var allMethodsLower = new SortedSet<string>(
            typeMap.Values.SelectMany(s => s).Select(m => m.ToLowerInvariant()),
            StringComparer.Ordinal);
        int methodCount = allMethodsLower.Count;

        Console.WriteLine($"Extracted {typeCount} types, {methodCount} distinct method names (lowercased, deduped).");

        // Resolve output paths relative to the repo root.
        // The generator lives at tools/gen-al-builtins/ — go up two levels from here
        // to reach the repo root, then navigate to the target files.
        string genDir = AppContext.BaseDirectory; // may be deep in obj/; use source dir instead
        // Walk up from the exe location to find the repo root (contains Cargo.toml).
        string repoRoot = FindRepoRoot(genDir) ?? FindRepoRoot(Directory.GetCurrentDirectory());
        if (repoRoot == null)
        {
            Console.Error.WriteLine("ERROR: Could not locate repo root (no Cargo.toml found).");
            return 1;
        }

        string rsOut = Path.Combine(repoRoot, "src", "program", "resolve", "global_builtins.rs");
        string jsonOut = Path.Combine(repoRoot, "tools", "gen-al-builtins", "out", "member_builtins.json");

        Directory.CreateDirectory(Path.GetDirectoryName(rsOut));
        Directory.CreateDirectory(Path.GetDirectoryName(jsonOut));

        // Emit global_builtins.rs
        EmitRustFile(rsOut, allMethodsLower, typeCount, methodCount);
        Console.WriteLine($"Wrote: {rsOut}");

        // Emit member_builtins.json
        EmitJsonFile(jsonOut, typeMap);
        Console.WriteLine($"Wrote: {jsonOut}");

        Console.WriteLine("Done.");
        return 0;
    }

    static string FindRepoRoot(string startDir)
    {
        if (startDir == null) return null;
        var dir = startDir;
        for (int i = 0; i < 20; i++)
        {
            if (File.Exists(Path.Combine(dir, "Cargo.toml")))
                return dir;
            var parent = Directory.GetParent(dir);
            if (parent == null) break;
            dir = parent.FullName;
        }
        return null;
    }

    static void EmitRustFile(string path, SortedSet<string> methods, int typeCount, int methodCount)
    {
        var sb = new StringBuilder();
        sb.AppendLine("// @generated — DO NOT HAND-EDIT.");
        sb.AppendLine("// Regenerate with: dotnet run --project tools/gen-al-builtins/gen.csproj");
        sb.AppendLine("//");
        sb.AppendLine("//! AL compiler-intrinsic GLOBAL method catalog.");
        sb.AppendLine("//!");
        sb.AppendLine("//! Provenance:");
        sb.AppendLine("//!   Source : Microsoft.Dynamics.Nav.CodeAnalysis.dll");
        sb.AppendLine("//!   AL ext : ms-dynamics-smb.al-18.0.2293710");
        sb.AppendLine("//!   Resource: ClassDocumentationResources");
        sb.AppendLine($"//!   Generated: 2026-06-13");
        sb.AppendLine($"//!   Types : {typeCount}");
        sb.AppendLine($"//!   Methods: {methodCount} distinct (lowercased, union of all types)");
        sb.AppendLine("//!");
        sb.AppendLine("//! Soundness rationale (bare-call path):");
        sb.AppendLine("//!   In AL, a bare call `Foo(...)` that is NOT resolved to a procedure in the");
        sb.AppendLine("//!   caller's own object must be a compiler-intrinsic global function — there is");
        sb.AppendLine("//!   no other bare-call target in the language.  The union of all method names");
        sb.AppendLine("//!   across the 97 compiler-documented types is therefore a SOUND allowlist: any");
        sb.AppendLine("//!   bare unresolved call whose name appears here IS a platform global.  No");
        sb.AppendLine("//!   static/member partition is needed on the bare path because bare syntax");
        sb.AppendLine("//!   (`Foo()` not `Obj.Foo()`) can only be a global.");
        sb.AppendLine();
        sb.AppendLine("use phf::phf_set;");
        sb.AppendLine();
        sb.AppendLine("/// Complete set of AL compiler-intrinsic global method names (lowercased).");
        sb.AppendLine("/// A bare call whose name is in this set is classified as `builtin`.");
        sb.AppendLine("pub static GLOBAL_BUILTIN_METHODS: phf::Set<&'static str> = phf_set! {");
        foreach (var m in methods)
        {
            sb.AppendLine($"    \"{m}\",");
        }
        sb.AppendLine("};");
        sb.AppendLine();
        sb.AppendLine("/// Returns true if `name_lc` (already lowercased) is a compiler-intrinsic global.");
        sb.AppendLine("pub fn is_global_builtin(name_lc: &str) -> bool {");
        sb.AppendLine("    GLOBAL_BUILTIN_METHODS.contains(name_lc)");
        sb.AppendLine("}");
        sb.AppendLine();
        sb.AppendLine("#[cfg(test)]");
        sb.AppendLine("mod tests {");
        sb.AppendLine("    use super::*;");
        sb.AppendLine();
        sb.AppendLine("    #[test]");
        sb.AppendLine("    fn catalog_spot_checks() {");
        sb.AppendLine("        assert!(is_global_builtin(\"guiallowed\"), \"GuiAllowed must be in catalog\");");
        sb.AppendLine("        assert!(is_global_builtin(\"strlen\"), \"StrLen must be in catalog\");");
        sb.AppendLine("        assert!(is_global_builtin(\"strsubstno\"), \"StrSubstNo must be in catalog\");");
        sb.AppendLine("        assert!(is_global_builtin(\"createguid\"), \"CreateGuid must be in catalog\");");
        sb.AppendLine("        assert!(is_global_builtin(\"format\"), \"Format must be in catalog\");");
        sb.AppendLine("        assert!(!is_global_builtin(\"thisisnotarealglobalxyz123\"), \"nonsense name must be absent\");");
        sb.AppendLine("    }");
        sb.AppendLine("}");

        File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));
    }

    static void EmitJsonFile(string path, SortedDictionary<string, SortedSet<string>> typeMap)
    {
        // Emit pretty-printed JSON: { "TypeName": ["Method1", "Method2", ...], ... }
        var options = new JsonSerializerOptions { WriteIndented = true };
        // Build a plain Dictionary<string, string[]> for serialization.
        var dict = new Dictionary<string, string[]>(typeMap.Count);
        foreach (var kv in typeMap)
            dict[kv.Key] = kv.Value.ToArray();

        string json = JsonSerializer.Serialize(dict, options);
        File.WriteAllText(path, json, new UTF8Encoding(false));
    }
}
