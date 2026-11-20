# ENG-152: interface implementers by identity

Task: `/coord/tasks/ENG-152/task.md`. Base: master `4ad2861a` (ENG-142, ENG-151 and ENG-148 in).
Design principles used (owner mantra): 1 and 2 (correct first), 5 (unknown is not no: an
unresolved or ambiguous name over-approximates, never drops), 13 (right-sized: namespaces are a
separate, engine-wide task), 16 (clean cutover: the name-keyed API goes), 19 (one home per fact),
30 (design reviewed before code). Revision 2 folds the design reviews of sol
(`pi-sol-eng152-design.md`) and Fable (alc 18.0.41 probes).

## The problem

`ResolveIndex::implementers` is keyed by the folded interface NAME, so two apps' same-named
interfaces pool their implementers: dispatch through app A's `IDup` also reaches app B's
implementers (wrong targets, wrong explain/prove paths). The model's dispatch meta, the impact
hub, the closed-enum and holder checks and G28/G29 are name-based too. Interface references read
from symbols or synthesized from a runtime package drop their `#<guid>#` app qualifier.

## What alc does (probed, Fable)

- Without namespaces, two visible same-named interfaces are an error wherever the name is used
  (AL0275); a workspace declaring its own is also AL0197: the own app does NOT shadow.
- One app cannot declare the name twice in one namespace (AL0197): (app, name) identifies an
  interface as far as the graph can see (it keeps no namespaces).
- With namespaces, a name binds per FILE (its `namespace`, then `using`, then global). The IR
  and the graph keep no namespaces, so that binding cannot be modelled here: filed separately.
- Symbols: an own-app reference is bare; another app's is `#<32 hex>#Name` (prefix outside the
  quotes); a return type names another app's interface by `Subtype.ModuleId` (dashed guid). An
  enum's `Implementation` is positional codeunit ids.

## Design

**D1. A typed interface reference.** `ObjectNode::implements` and `interface_bases` become
`Vec<InterfaceRef>`: `{ name: SharedStr, app: Option<AppGuid> }`. `app` is set only from symbols
(`#<guid>#`, `ModuleId`) and from synthesized text; it is parsed ONCE where the text enters the
graph (`node_extract`, `abi_ingest`). The symbol reader keeps the qualifier (not strips it).
Synthesis writes it as a qualified name, `implements "#<guid>#"."Dep Iface"`, which the grammar
takes and which every unchanged name reader sees as `Dep Iface` (the lowerer's `reference_name`
keeps the last segment); the lowerer additionally keeps the qualifier for `node_extract` to read.
The model's name-based `implements_interfaces` (d19, census) keeps the bare name.

**D2. One binding rule** (the graph, beside `resolve_object`):

    ProgramGraph::bind_interface(&self, from: AppRef, r: &InterfaceRef) -> InterfaceBinding
    enum InterfaceBinding { Exact(ObjectNodeId), Visible(Vec<ObjectNodeId>), Unbound }

- `r.app` set: the interface of that name in the app with that guid (dashes ignored, any case):
  `Exact`. That app or name not in the program: `Unbound` (never rebind it to another app's).
- Bare: the interfaces of that name visible from `from` (its own app and its dependency closure).
  One: `Exact`. Several: `Visible(all)`, an over-approximation (alc refuses this without
  namespaces; with namespaces the file's `using` would decide, which the graph cannot see). None:
  `Unbound`.
- `Unbound` keeps today's reach: an implementer whose reference is unbound is filed under the NAME
  bucket (D3), and a receiver whose type is unbound reaches that bucket. Nothing that is reached
  today is dropped.

**D3. The index keyed by identity.** `implementers` and `closed_enum_implementers` become
`HashMap<ObjectNodeId, Vec<ObjectNodeId>>`. Each implementer is filed under every interface its
references bind to (`Exact` or each of `Visible`) and, transitively, their bases (bases bound from
the declaring interface's app by the same rule, as ENG-142 does). An `Unbound` reference files it
under `unbound_implementers: HashMap<name, Vec<ObjectNodeId>>`. Queries:
`implementers_of(&ObjectNodeId)`, `unbound_implementers_of(name)`, and
`unmapped_enum_implementers(graph, &ObjectNodeId)`. The name-keyed forms are removed.

**D4. Bind where the type is declared.** `ReceiverType::Interface { name_lc, binding }`: the
binding is made in `receiver.rs` where the parsed type becomes a receiver type (~3234), from the
DECLARING object (a local, parameter or own global: the routine's object; an inherited protected
global: its declaring object; a callee's return type: the callee's object, and its `ModuleId` when
it is a symbol). The resolver's fan-out is the union of `implementers_of` over the binding (plus
the name bucket when `Unbound`), deduplicated in a stable order. Interface member signatures
(`receiver.rs` ~2263-2288, `arg_dispatch.rs` ~1716) read the bound interface's routines. Callers
by identity: `applicability.rs` (takes the site's binding; drops its own by-name match; its
`semantic_golden.rs` caller passes the site's binding), `arg_dispatch::interface_binds` (param is
an id), `full.rs` G24/G28/G29 (the receiver's binding; G28's name-global `unique_object` goes).

**D5. Closure checks by the same identity.** An enum's mapping keys (`enum_impls`) are interface
references bound from the enum's app (D2); `closed_enum`, `index.rs` ~1120 and
`interface_is_or_derives` compare that binding to the receiver's through the base relation, and
an `Unbound` key counts as possibly this interface (as today's `_ => true`). Interface holder
membership (`interface_holder.rs` ~253-266) and literal reach (`literal_reach.rs` ~2107, 2496) the
same. `graphify_export.rs` ~315-330 exports families per interface, not per name.

**D6. Model meta and impact hub.** Site facts carry the receiver's binding (`interface_ids`,
beside `interface` kept for display). The model meta's codeunit implementers are
`implementers_of` over those ids (the index already propagates to bases: no reverse family walk);
its enum list, which must include open enums and enum extensions, uses D2 over each enum's
references. The display-name pick and G29's `publisher_scoped` use the bound interfaces. The
impact hub seeds the interfaces declared in its apps and reads `implementers_of` directly.

**D7. Session replay.** A new recorded read, `Query::BindInterface(app, InterfaceRef)`, replays
the binding outcome (including `Visible` and `Unbound`), and `ImplementersOf(ObjectNodeId)` /
`UnboundImplementersOf(name)` replay the buckets. `FACADE_VERSION` bumps. A test adds a third
same-named interface in a declaration-level refresh and compares warm with cold.

**D8. Caches.** `CACHE_VERSION_SYMBOL_READER` (the reader keeps the qualifier) and
`EXTRACTION_SCHEME` (synthesized text changes) both bump; the pack schema if the graph's node
shape is packed (checked during implementation).

## Revision 3 (sol, round 2)

- **Unbound keeps today's reach (sol and Fable, same P1; Fable's rule).** A BARE reference with
  no interface of the name visible binds `Visible(every interface of that name in the program)`,
  as the ENG-142 fallback does today (a missing topology edge, e.g. an unmatched dependency, must
  not lose targets). `Unbound` means only: no interface node of that name exists at all, or a
  QUALIFIED reference whose app or name is missing. For those the name bucket is exact (nothing
  bound can be the same interface), keyed `(Option<guid>, name)`, so `#A#I` and `#B#I` never
  pool. The model meta (D6) asks the same candidate query as dispatch. Test: an interface loaded
  but not visible (no topology edge) still reaches its implementer.
- **Carrier in the IR (Fable P3).** `ObjectDecl::implements` and `interface_extends` stay bare
  names (every current reader, and the model's `implements_interfaces` for d19 and the census,
  unchanged); new parallel `implements_qualifiers` / `interface_extends_qualifiers:
  Vec<Option<String>>` hold a reference's leading segments (`"#<guid>#"`, or a namespace, which
  stays unused until ENG-158). Type text keeps its qualifier the same way: `ParsedType::Interface`
  gains the qualifier, read by `classify_type_text` before `strip_namespace`.
- **Symbols prove identity.** A bare reference in a symbol file means the package's own app
  (alc writes another app's qualified), so `abi_ingest` stamps it with that app, and so does
  `node_extract` for a SYNTHESIZED file (`transpiled/` from a runtime package), whose text comes
  from the same symbols. Only hand-written source (namespaces erased) yields `Visible`.
- **Non-exact never proves.** `Visible` and `Unbound` may widen routes and enum coverage
  obligations; they never prove membership, pick an overload, or close a site (closed-enum,
  holder, G28/G29 treat a non-exact receiver as open).
- **Every type reader.** Also a call result used as an argument (`arg_dispatch.rs` ~1881-1900:
  the return type binds from the callee's object), and synthesized signatures carry another
  app's interface type qualified from `ModuleId` (`synth.rs` `type_text`), not only headers.
- **Verifier.** `semantic_golden.rs`'s re-walk (~1051-1067) takes the site's recorded binding
  (it cannot re-type `Dep.Make().Go()`).
- **Tests.** A synth -> lower round trip for a long escaped name and a qualified enum mapping.
  D7's warm/cold test must fail with `BindInterface` recording removed: change a recorded
  binding while its old implementer buckets stay the same.

## Tests (each with a red proof)

- Two dependencies A and B, neither seeing the other, each declare `IDup` with an implementer; the
  workspace sees only A: a call through `IDup` reaches only A's implementer (today: both).
- A call inside B through B's `IDup` reaches only B's.
- A workspace that sees both (ambiguous): both (over-approximation kept).
- Return type declared elsewhere: `Dep.Make().Go()` binds to `Make`'s interface, not to a
  same-named one visible from the caller's app.
- ENG-142's closure by identity: a derived interface's implementer reaches its own base, not a
  same-named base of another app.
- Qualified: a symbol-only package and a synthesized one, each declaring its own `Dep Iface` and
  implementing A's `#<A>#"Dep Iface"` (which extends `IBase`): a call through A's `IBase` reaches
  their codeunit; a missing qualified app stays unbound (reached only through the name bucket).
- Enum closure: a closed enum whose mapping key binds to the other app's `IDup` does not close a
  site on this one.
- Model meta (callees and unresolved_impls) and impact hub over the two-app shape.
- Session: warm equals cold after a third same-named interface appears.

## Not in scope (filed as ENG-158)

AL namespaces (ENG-158, engine lane): object `namespace`, file `using`, qualified source
references, per-file binding.
It changes every object reference, not only interfaces. With it, D2's `Visible` case becomes
exact for namespaced code.

## Measure

CDO/DO before/after: findings, fingerprints, `aldump --program-call-graph-stats`, and the route
count on interface edges (a pooled target dropped shows as fewer routes). Expect few or none;
explain every move.
