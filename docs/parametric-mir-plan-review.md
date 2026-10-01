# Review of the parametric MIR plan

Assessment dated 2026-09-30, against [the plan](parametric-mir-plan.md),
the current worktree, and Modular's compiler sources at
`e700d92fe1ba720701d5b9d233c791590199bfb1`. This is a separate design review,
not a replacement plan or a record of implemented changes. The worktree
already contains census work; the plan's statement that nothing has landed
should not be taken as a description of those uncommitted changes.

**I support the direction, but would strengthen the design before committing
to P2 and P3 as written.** Moving specialization onto checked parametric IR
addresses the cause of the AST-cloning and fact-derivation complexity. Using
one elaborator for the VM and native backends is a useful first step with
independent value. Keeping the AST, `CheckedProgram`, and Rust MIR is a
reasonable way to pursue Mojo's phase ordering without reopening the rejected
Pliron core experiment.

The weakness is that the plan treats some substantial semantic work as an
extension of substitution. P1 has a concrete implementation to build on;
P2–P4 need a specification for what checking proves, what remains conditional,
and how elaboration discharges those conditions. Otherwise the certificate
machinery could reappear inside MIR under different names.

The upstream direction is correctly identified: lifetime checking and
destructor insertion precede generator elaboration, which performs
substitution, compile-time evaluation, and static assertion checks. That is
the architectural resemblance worth preserving.
([Upstream walkthrough](https://github.com/modular/modular/blob/e700d92fe1ba720701d5b9d233c791590199bfb1/Mojo/docs/compiler/MojoCompilerWalkthrough.md))

1. **Specify the generator contract before removing clone checks.**

   “Every instance-dependent decision is an explicit MIR operation” is a
   good principle, but it needs an inventory and a preservation argument.
   A generator should name its binders, assumptions, deferred obligations,
   selected calls and witnesses, lifecycle requirements, and origin/transfer
   summaries. Its source locations and instantiation provenance must survive
   substitution. Reuse `ParamRef`, `ParamExpr`, `ParamConstraint`, and the
   existing callable-identity machinery rather than creating parallel
   vocabularies.

   The intended guarantee should be: **substituting arguments that satisfy
   the recorded obligations preserves typing, ownership, and effects; no
   source expression is inferred or overload-ranked again.** “Check once”
   should mean one semantic checking process per declaration, not literally
   one traversal. Recursive effect summaries and loop dataflow can still
   require fixed points within that process.

   This matters beyond `where` clauses and `rebind`. The existing
   [instance obligations](notes/instantiation-from-template.md#what-an-instance-still-owes)
   include implicit copying, deletability, reference reads, loans in stored
   values, closure effects, and replayed transfers. Some currently vary
   with the substituted type. P2 explicitly includes lifecycle methods, so
   this cannot all wait for P3.

   Classify each existing obligation as a declaration-time proof, an
   assumption supplied by a bound, an operation or summary retained in IR,
   an instantiation-time predicate, or obsolete clone bookkeeping. The
   instantiation note is a migration inventory, not the permanent generator
   specification: clone retargeting and local renumbering should disappear,
   while real language requirements survive independently of body shapes.

   One particularly useful audit target is
   [`Specializer::speculative`](../crates/mojito-native/src/native/mono.rs).
   Today it tolerates failing eagerly discovered constructors because their
   availability clauses are absent from MIR and checked callers are trusted.
   A general elaborator needs explicit availability information and demanded
   versus speculative reachability before the checker work it relies on is
   removed.

2. **Make ownership and compile-time regions the first design experiment.**

   “Ownership analyses every arm” does not specify the join rule, last-use
   placement, branch-local assumptions, or cleanup on return and raise.
   “Analyse the loop body once” similarly leaves loop-carried ownership,
   zero iterations, heterogeneous pack elements, and compile-time exits open.
   Reusing the shape of `Try` can help representation, but does not supply
   those semantics.

   There is useful implementation evidence to study directly:
   [upstream `CheckLifetimes.cpp`](https://github.com/modular/modular/blob/e700d92fe1ba720701d5b9d233c791590199bfb1/Mojo/lib/LowerLIT/CheckLifetimes.cpp)
   handles compile-time conditionals in its if-like scans and computes a
   stable consume set for compile-time loops. The problem is therefore not
   whether symbolic lifetime checking is possible in principle; it is how
   to express the necessary facts in Mojito's ownership model.

   Before broad P2 deletion, make a narrow vertical experiment cover a
   move-only value, a reference-bearing value, a conditional use followed by
   a use after the join, and destruction across an early exit. For P3b, add
   zero-, one-, and multiple-iteration cases and a heterogeneous pack. Record
   the pinned Mojo verdict and destructor order, including negative cases.
   These are proposed probes, not claims that particular untested programs
   are accepted upstream.

   Separate symbolic ownership legality from instantiating lifecycle glue.
   Substitution may resolve a destructor witness or expand aggregate cleanup;
   that should not silently recompute source-language last-use semantics.
   A development-only concrete ownership comparison is useful, but ordinary
   `mir::verify` alone does not establish ownership preservation.

   The post-elaboration ownership fallback needs a tighter condition. It may
   be a temporary implementation strategy, but it must preserve required
   rejection of invalid untaken arms and unused declarations. If it checks
   only the selected instance and consequently accepts more programs, it is
   a language-semantic change as well as a phase-order divergence.

3. **Design CTFE's connection to the worklist before P3, and decide D3 early.**

   CTFE already using the VM does not mean it already consumes the proposed
   generator representation. In
   [`comptime/ctfe.rs`](../crates/mojito-comptime/src/comptime/ctfe.rs),
   evaluation builds AST subprograms, checks a synthesized typing probe,
   converts the result type back to source syntax, and executes another
   synthesized helper. Keeping that route indefinitely would preserve
   checking and AST reconstruction inside the new elaborator.

   Specify a request path now: a typed compile-time application demands a
   concrete callable from the shared worklist, runs verified MIR on the VM,
   and returns a validated compile-time value. Define how evaluation can
   demand further instances without recursively restarting the source
   pipeline. Keep the existing effect restrictions, value-crossing rules,
   and shared fuel accounting.

   The worklist needs canonical keys and states for pending, active,
   completed, and failed requests. Distinguish valid recursive function
   references from a cycle that demands an unfinished constant or layout;
   bound expanding polymorphic recursion separately from VM instruction
   fuel. Existing `native::mono` identity and instance-budget machinery is
   the starting point, not a reason to invent another instantiator.

   D3 affects signatures and types, not just module initialization. A
   module constant can depend on a generic compile-time call whose result
   shapes another declaration. Before P3, decide which closed constants may
   be folded early and which dependencies remain represented as typed
   requests. An early-folding implementation can stay temporarily, but P4's
   promise of checking once needs a defined boundary now.

   D2 is reasonable for P1: the root can call `native::mono` for both
   backends. Before connecting CTFE, draw the actual call and crate graphs.
   The current comptime crate does not depend on the native crate; a later
   connection or extraction needs the repository's explicit dependency
   design decision. Renaming the crate at P5 will not resolve this by itself.

4. **Define phase legality explicitly, and give the artifacts clear names.**

   Parametric verification must accept well-scoped, well-kinded binders and
   explicit residual obligations. It must still reject unbound parameters,
   unresolved inference holes, malformed call contracts, and missing facts.
   A generic “symbolic types allowed” switch would weaken the waist.

   Concrete verification must inspect more than register types: declaration
   fields, signatures, instruction payloads, constants, nested regions,
   parameter applications, witnesses, and obligations all matter. State what
   metadata may survive without executable meaning. Also distinguish erased
   generic dispatch from legitimate indirect calls and runtime closures;
   eliminating the former does not eliminate the latter.

   Use separate phase wrappers or equally explicit API contracts for checked
   parametric MIR, ownership-verified MIR with drops, and concrete executable
   MIR. They can share representation internally. This avoids requiring
   callers to remember which verifier mode happened to run.

   This distinction is already needed at P1. Today
   [`CompiledProgram::elaborated_mir`](../src/compiler.rs) means
   *drop-elaborated* MIR, which can still be generic. The roadmap also says
   serialized artifacts remain pre-specialization and `exec` specializes
   them. Specify those boundaries, cache the concrete graph appropriately,
   and prevent the native backend from specializing it a second time.
   Define entry roots consistently for `main`, top-level execution, artifact
   execution, CTFE, and externally requested native entries.

   Update the architecture and public contract documentation when P1 changes
   them. Deferring the rewrite until P5 would leave authoritative documents
   describing the wrong production pipeline for several stages.

5. **Adjust P3's dependencies to match the representation work.**

   P3b's heterogeneous packs already need types dependent on a symbolic
   index. P3c cannot be the first point at which dependent register types
   receive a coherent specification. Likewise, P2's generic struct methods
   need receiver applications, conditional members, and lifecycle witnesses
   before P3d supplies full value-keyed struct generators.

   Introduce the minimal common type and generator vocabulary first. Then
   split ordinary value-indexed `comptime for` from heterogeneous pack
   expansion, and split P3e's method parameters, nested definitions/captures,
   and CTFE integration. They share infrastructure but have different
   correctness conditions. Keep a mixed-feature probe through the stages so
   individually successful migrations also compose.

   Restrict layout requests to concrete types, as proposed, while specifying
   the target environment used by compile-time layout queries. A concrete
   type does not make a layout query independent of the target.

6. **Treat P1's evidence as encouraging, not a completed parity proof.**

   The [A1 coverage record](notes/pliron-a1.md#coverage) distinguishes full
   corpus conversion from VM execution checks on focused inputs. The
   historical `scripts/cover-pliron-a1` at `1967fe1d` invokes the harness
   with `--phase compile`; its 893 successful rows should not be described
   as 893 runtime equivalence checks. Restore the proposed parity gate
   before switching the default.

   Compare results, output, error categories, and ordered lifecycle events,
   with controlled inputs for programs that read files or stdin. Include
   artifact round trips, ownership/type-error probes, and failures of
   instantiation constraints. Execution parity on accepted programs cannot
   detect a checker that now accepts invalid programs.

   Keep the erased VM as a migration comparator, but use the Mojo pin to
   adjudicate known Mojito divergences. For example,
   [overload selection](notes/instantiation-from-template.md#overload-selection-is-bound-once)
   must remain bound at the template check even where clone checking used
   to choose differently.

   The full corpus belongs in the prescribed nightly or heavy lanes as
   appropriate. Stage development can use focused probes; this review does
   not recommend running broad sweeps interactively.

7. **Keep the performance discipline, but revise D4 and the stop rule.**

   Requiring every historical performance row to improve is not a reliable
   acceptance rule. The document contains several measurement generations,
   and removing discovery rounds does not guarantee every workload speeds
   up: eager specialization can add work and increase code size.

   Freeze a specific P0 revision, build configuration, machine, workload
   set, and timing boundaries. Measure compilation separately from execution
   and use repeated measurements with a stated noise tolerance. Track peak
   memory, instance counts, and generated IR size alongside elapsed time.
   Add compile-time branches/loops/packs and many-instantiation workloads;
   roadmap 1.6 already identifies the missing P3 benchmark coverage.

   The 1.20 ceiling is a reasonable proposed regression alarm, not evidence
   that 20 percent is universally the right tradeoff. Set a clear required
   improvement on the workloads dominated by repeated checking, plus
   bounded regressions elsewhere. Measure with differential verification
   disabled when deciding production performance.

   If P1 misses its budget, keep the old default while investigating
   reachability, copying, and instance caching. A slow first implementation
   would not establish that the architectural goal belongs in non-goals.

8. **Correct the P6 analogy, and design package requirements before P6.**

   The upstream walkthrough says precompilation runs semantic checks and
   rebuilds a `lit.package` carrying post-parse bodies, before `LowerLIT`.
   That is not the post-`LowerLIT` KGEN generator artifact described by the
   normal pipeline. Serializing Mojito's drop-elaborated generators may be a
   good choice, but it should be presented as Mojito's chosen boundary,
   rather than exactly what `.mojoc` stores.
   ([Upstream package flow](https://github.com/modular/modular/blob/e700d92fe1ba720701d5b9d233c791590199bfb1/Mojo/docs/compiler/MojoCompilerWalkthrough.md#package-creation-flow))

   Package implementation can remain last. Its metadata requirements should
   influence the generator design now: exported signatures, generic bodies,
   conformance and effect summaries, dependency identities, source
   provenance, compiler/schema compatibility, target assumptions, and cache
   invalidation. A package must preserve templates that the producing
   program's entry graph does not reach. An entry-pruned executable artifact
   is insufficient.

   Do not promise a single schema bump before all P3 forms are designed.
   Either reserve a complete format now or permit further versioned changes.
   Importing a package can avoid source rechecking while still requiring
   artifact validation and instance-obligation checks.

**Suggested sequence.** Keep P0 and P1, adding explicit verifier and artifact
contracts. Before broad P2 removal, write the generator-obligation inventory
and prove a small ownership/control-flow example through the complete path.
Design the CTFE request protocol and module-constant dependency handling at
that point. Then migrate P2 and the smaller P3 classes against those contracts,
merge the checks at P4, remove obsolete mechanisms at P5, and ship package
caching at P6.

Keep the freeze on expanding the certificate mechanism, but allow correctness
fixes to existing behavior. Treat deletion as the completion condition for a
migrated class, with the comparison path explicitly temporary. “Every stage
deletes its replacement” and “keep the erased oracle until P5” need that
distinction. A monotonically decreasing line count is less useful than
tracking which semantic decisions still require cloning or inference.

The next deliverable I would request is a short generator contract plus the
focused ownership and CTFE experiments, alongside P1's parity work. Those
would turn a good architectural direction into a plan with much clearer
implementation risks and completion criteria.
