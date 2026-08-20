# Pliron Stage 4: References, Destruction, and Exceptional Control Flow

Stage 4 makes the feature-gated Pliron backend consume drop-elaborated MIR
exactly as emitted: tagged success/error outcomes with explicit CFG edges,
structural `try`/`except`/`else`/`finally` lowering, references as verified
place addresses, invisible-release destruction semantics matching the VM's
frame model, and ordered lifecycle-event instrumentation in the test lane.
The normative contract is [`docs/native-abi.md`](../native-abi.md) (ABI
version 3); this note records the design decisions and divergences.

## The raising-function ABI

- Every `raises` function compiles as `void f(outcome*, params...)`: one
  prepended, caller-allocated outcome pointer, uniformly (no scalar fast
  path — that is Stage 6 optimization work). The outcome replaces the plain
  sret slot when the return type is an aggregate: `{ tag: u32, ok: T, err:
  MjError }` laid out by `native::layout::outcome_layout`, the ok payload
  inline. A function never receives both pointers.
- A raising call site allocates outcome storage, calls, loads the tag, and
  branches: the error edge byte-copies the callee's `MjError` into the
  function's single error staging slot and jumps to the innermost raise-edge
  target; the ok edge extracts the payload and continues — so post-call
  effects (a `mut self` receiver write-back) run only on success, exactly
  like the VM's `store_at_call_place`.
- The raise-edge target is the innermost enclosing `try`'s landing block,
  else the function's propagate block: free the buffers of still-initialized
  releasable locals (no user destructor — the VM truncates raising frames
  and its arena reclaims memory invisibly; abandoned locals of other
  droppable types remain a recorded leak residue), move the staged error
  into the outcome, tag it, return. The executable wrapper allocates the
  outcome for a raising `main`/`__toplevel__` and reports a propagated error
  through `mjrt_unhandled_error` — stderr and exit 69 unchanged from the
  Stage 3 in-callee report. The JIT refuses raising entries.

## Initialization flags

Drop elaboration legitimately drops not-yet-initialized slots (ahead of
`try` regions) and lists variables on cleanup edges they already died before;
the VM's empty-slot drop is a silent no-op and that idempotency is
load-bearing. Natively, every droppable variable carries an `i1`
initialization flag (parameters start true): set on `DefVar` and
whole-variable stores, cleared on drop/consume/move-out, and every drop —
`DropVar`, `Try.cleanup`, escape and return edges, propagate-path releases —
is flag-guarded. Droppable non-parameter slots also zero at entry
(`llvm.memset`): guarded paths never read undefined bytes, and the intrinsic
use keeps pliron's mem2reg from promoting a slot whose only stores sit in
since-pruned unreachable blocks (promotion materializes an element-typed
poison — the `i8`-array alloca trap; typed scalar staging slots use typed
allocas for the same reason).

## Try flattening

- `MirInstr::Try`'s four region mini-CFGs (which share the function's
  register and variable space, with region-local block ids) flatten into the
  function's flat block list. `FallOff` jumps to the region's continuation;
  raises inside the body jump to a per-try landing block.
- `Try.cleanup` runs on **every** body exit — raise, normal completion,
  return, escape — mirroring `exec_try`. The landing block runs the cleanup
  drops, then binds the staged error into the handler variable (freeing a
  still-initialized previous binding first — a loop rebinding the same
  handler var) or frees it for binder-less handlers, then enters the handler.
  `else` runs only on normal completion, after the cleanup drops.
- Raise lowering leaves the dead remainder of its MIR block in an
  unreachable block; unreachable blocks are pruned with pliron's
  `simplify_cfg::remove_blocks_inside_op` before verification (the dominance
  verifier does not tolerate value uses inside unreachable blocks).
- Owned-temporary positions extend into regions: the last-use walk and the
  region lowering assign synthetic block ids in the same
  body → handler → orelse → finalbody order, so the Stage 3 release rule
  works unchanged inside regions.

## `finally`: single instance, pending-outcome dispatch

Each `finally`-bearing try lowers its finalbody **once**, with a per-try
`i32` pending-kind slot (0 normal, 1 error, `2 + site` for a function-exit
site) and per-try pending-error storage (the shared staging slot may be
clobbered by a raise handled inside the finalbody itself). Handler and
`else` regions lower under a pseudo-frame so their raises pend an error on
the finalbody and their returns cross it, without re-running the body-edge
cleanup. A return or escape crossing frames stages its value at the site
(into the outcome/sret, or a typed scalar staging slot), registers an exit
site, runs enclosing cleanups inner-to-outer, and pends on the first
finalbody in the way; the post-finalbody dispatch switches on the pending
kind and forwards — continuation, re-raise toward the enclosing observer, or
onward crossing. The dispatch emits an error case only when an error can
actually pend (`error_possible`), so a raise-free `try`/`finally` in a
nonraising function needs no propagation path.

A finalbody's own raise, return, or escape simply never reaches the dispatch
— the VM's "finally outcome wins" — and resolves the overridden pending
outcome: a pending return's carried cleanup roots still leave scope (at the
overriding return's terminal, after its own roots — the VM's distinct-union
merge, made idempotent by the flags), a pending error's message frees. The
resolution switches recurse through nested overrides statically.

## Errors as values

`Ty::Error` lowers as `MjError` aggregate storage. Copies deep-copy the
message (the VM's clone duplicates its string, so a copy outlives the
original's drop); drops and releases free `message.data` invisibly — the
built-in error has no user destructor; `print` and `String`-context reads
use the `(data, size)` pair. A bound-but-unused handler error receives no
MIR drop (the VM abandons it to its arena at frame end), so the landing
frees a previous binding before rebinding and every function return runs
flag-guarded frame-exit releases for error-typed locals (borrowed parameters
excluded — their value belongs to the caller). This stays scoped to errors:
byte-copied aggregates (`LoadPlace`) share buffers by design, so a general
frame-exit release would free returned storage.

## References

- `Ty::Ref` is one opaque pointer (`ScalarTy::Ptr`); ownership facts erased
  after validation, never re-derived.
- `mut`/`ref` parameters pass the **address of the caller's designated
  storage** (the call's `arg_places` at reference positions; keyword-bound
  reference arguments stay rejected) and the callee's variable slot aliases
  the incoming pointer — reads and writes are write-through, extending the
  Stage 3 aggregate-parameter aliasing to any parameter kind.
- `MakeRef` materializes a place address; `ReadRef`/`WriteRef` load or store
  through the handle (aggregates copy out, the VM's clone-on-read);
  `StoreRef` stores the handle into reference-typed storage. A place
  `through` a local reference loads the stored handle and projects relative
  to the referent; a place through a `mut`/`ref` parameter (typed as its
  referent) uses the aliased slot directly. `BorrowShared`/`BorrowMut`
  variable uses are the slot address.
- A reference-returning function returns one pointer (`RetKind::Ptr`, JIT
  refused); `raises` plus a reference return is rejected (no outcome layout
  for a referent-typed ok slot yet). `result_adapter` method contracts stay
  rejected (Stage 5 iterator machinery).

## Lifecycle instrumentation

Under `CompileOptions::trace_lifecycle` (test lane only — default emission
never traces), the backend calls `mjrt_trace` (one `mjtrace <kind>
<payload>` line on stderr, so stdout parity is untouched) at: compiled
`__deinit__` dispatches (`drop <Type>`), `ConsumeVar` (`consume <Type>`),
raise sites (`raise <message>`), and handler entry (`catch <message>`).
Invisible releases and owned-temporary frees are deliberately untraced — the
VM has no corresponding event. The VM records the same four events in a
test-only ordered log (`VmBackend::enable_lifecycle_log`), and
`lifecycle_event_traces_match_the_vm` compares the ordered sequences.

## Recorded divergences and residues

- No user destructor runs on the error-propagation path (both backends);
  native frees only releasable buffers there — abandoned locals of
  user-`__copyinit__` types other than String/Error remain a recorded leak
  residue, as in Stage 3.
- A raising `mut self` method called through a borrowed receiver alias
  mutates caller storage up to the raise point natively, where the VM's
  copy-in/copy-out discards partial mutation on the error path (no current
  fixture observes this).
- The dynamic-residue rejections stay: destructor-with-droppable-fields,
  partially-moved drops, and consume-with-droppable-fields (the VM tracks
  `Value::Moved` tombstones the backend does not).
- Stage 3's divergences (allocation-count traps, discarded VM stdout on
  unhandled raises, alloc/stdout trap categories) carry forward unchanged.

## The acceptance harness

`tests/pliron_backend_test.rs::parity_exe_manifest_and_differential`
(named `stage4_exe_manifest_and_differential` when Stage 4 landed)
generates `conformance/pliron-parity.tsv` over `assets/ok` **and**
`assets/ownership_ok`: every fixture with a `main` either compiles natively —
exe stdout at `O0`/`O1` equal to the VM byte-for-byte and an
AddressSanitizer/LeakSanitizer-clean `O0` run, handled-raise and `finally`
paths included — or records `excluded` with its first diagnostic;
`assets/runtime_error/pliron_raise_*` fixtures compare the unhandled-raise
exit category and stderr. Shrink guards pin the eligible counts and forbid
`pliron_*` fixtures from regressing. New fixture families:
nested re-raise, `finally` overriding pending returns and errors, loop
escapes through `finally`, early returns, handled-and-dropped errors,
raising explicit destructors with rollback, reference write-back, and
raising-edge cleanup drop order. Negative ownership fixtures are verified to
fail in the front end before any backend runs; the ordered lifecycle-trace
differential closes the create/drop/error acceptance.
