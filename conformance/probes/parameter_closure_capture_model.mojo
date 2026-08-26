# PROBE (open divergence): the `@__parameter` closure capture model.
#
# At `a79fbdf59f2` (2026-08-26), upstream:
#   - runs this program (implicit MUTABLE capture of `total` under
#     `@__parameter`, no capture list) and prints 5;
#   - REJECTS an explicit capture list on a `@__parameter` def
#     (`def bump[n: Int]() {mut total}:` -> "expected ':' in function
#     definition"), while a plain nested def still takes one.
# Mojito is the mirror image: this program rejects ("expression must be
# mutable in assignment"; the inferred parametric-closure environment
# defaults captures to imm), while the explicit `{mut total}` spelling runs
# and prints 5 under either decorator spelling.
#
# Run:    mojo run parameter_closure_capture_model.mojo
#         cargo run -- run conformance/probes/parameter_closure_capture_model.mojo
#
# Follow-up when tackled: align Mojito's `@__parameter` closures to implicit
# mutable capture (and reject the capture-list combination), then update the
# functions.callable-values / ownership.closures parity rows and promote this
# to a cases.tsv claim.
def main():
    var total = 0

    @__parameter
    def bump[n: Int]():
        total += n

    bump[5]()
    print(total)
