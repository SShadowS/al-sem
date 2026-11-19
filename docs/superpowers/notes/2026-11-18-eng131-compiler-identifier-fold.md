# ENG-131: one compiler-exact identifier fold (2026-11-18)

## The defect

Graph node ids and name binding folded identifiers with `al_syntax::fold_identifier` (a per-char
lowercase); stable routine ids folded with `str::to_lowercase` (Unicode's full, context-sensitive
lowercase). The two differ on `İ` and on a word-final `Σ`, so two procedures such as `"ΒΣ"` and
`"Βς"` were two graph nodes that shared one stable id (found by Fable reviewing ENG-90; repro on
tools commit 536d7f69: `alsem query touches --from Root --direction up` read `local` 7 times and
`via-hub` 5 times over 12 runs).

## The compiler's answer (step 1)

alc 18.0.41.62505 (`/opt/al-tool`, .NET 10.0.15), compiled locally in the kraken container
(Linux) against `/work/src/cdo-baseline/Cloud/.alpackages`. Compile-only: identifier binding is a
compile-time question. Linux results; Windows confirmation pending. Probe apps: `eng131-probes/`
beside this note (`dup-*`: two procedures of the pair in one codeunit, AL0440 = one identifier;
`call-*`: a call by the other spelling, AL0118 = two identifiers).

| pair | alc | old `fold_identifier` (graph, binding) | old `to_lowercase` (stable ids) |
|---|---|---|---|
| "Ix" / "ix" (control) | one (AL0440) | one | one |
| "ΒΣ" / "Βς" (final ς) | one (AL0440); `"Βς"()` binds to "ΒΣ" | two (σ≠ς) | one (final-sigma rule) |
| "Σx" / "ςx" (non-final ς) | one (AL0440) | two | two |
| "İx" / "ix" | two (compiles); `"ix"()` → AL0118 | one (İ→i) | two |
| "aß" / "aẞ" | two | one (ẞ→ß) | one |
| "Kx" (U+212A Kelvin) / "kx" | two | one (K→k) | one |
| "ıx" (U+0131) / "ix" | two | two | two |
| "µx" (U+00B5 micro) / "μx" (U+03BC) | one (AL0440) | two | two |

Reading: alc compares like .NET `StringComparison.OrdinalIgnoreCase`: each char's simple UPPERCASE
mapping (ς, σ → Σ; µ, μ → Μ; ß and ẞ stay apart), except that no non-ASCII char folds onto ASCII
(ı and the Kelvin sign stay apart from i and k; İ stays İ). Neither old fold matched.

## The fix (step 2)

`fold_identifier` / `eq_fold_identifier` now fold each char to its class under that rule
(`casing::fold_char`), written as the class's single lowercase char when one maps back to the
class, else as the uppercase char. ASCII is unchanged (`to_ascii_lowercase`), and so is every char
whose lowercase was already its class (all Latin letters in real BC identifiers). The stable
routine id (`engine::ids`: the routine name, enclosing member, parameter names and type text)
folds with the same function, so binding, graph node ids and stable ids agree with the compiler
and with each other.

What moves: only names with a char whose class differs between the folds: `İ`, `ẞ`, the Kelvin,
Ohm and Ångström signs, `ς`/`Σ`, micro `µ`, `ı`/`ſ` (binding only). The counts on CDO are in the
CHANGELOG entry.
