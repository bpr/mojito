# PROBE (message/behavior pinning): what exactly does the audited head do for
# an invalid contiguous List slice bound on the CPU-default assertion
# configuration — abort text, exit behavior, and whether `try`/`except` can
# observe it?
#
# Context: nightly §5 (docs/mojo-nightly.md) requires contiguous List/Span/
# String slices to reject negative, out-of-range, or reversed bounds; on the
# CPU-default assertion configuration invalid bounds abort. Mojito models this
# as the uncatchable `RuntimeError::Abort` rendered as
# "abort: List slice bounds out of range" (stdlib/std/collections/list.mojo,
# the strict `__getitem__(slice: ContiguousSlice)` overload). The exact
# message is Mojito-chosen pending this probe.
#
# Run:    mojo run contiguous_slice_abort_message.mojo
#         cargo run -- run conformance/probes/contiguous_slice_abort_message.mojo
#
# If Mojo aborts with a distinct message: align the stdlib abort message and
#   the `# expect:` lines in assets/runtime_error/list_contiguous_slice_*.mojo
#   (message text only; the uncatchable-trap model stands).
# If Mojo raises a catchable Error instead: change the strict overloads to
#   `raises` and move the fixtures' outcome accordingly (model change, record
#   in parity row types.slices).
# If Mojo accepts and normalizes: §5 is stale for this claim — re-audit the
#   nightly doc before touching the strict overloads.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[0:9])
