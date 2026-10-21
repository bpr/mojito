# PROBE: a lambda called in an augmented assignment's value.
#
# The pin runs it and prints:
#   1
#
# Mojito: "compiler invariant violated: lambda reached inference before
# scoped checking" (R1231).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run augmented_lambda_value.mojo
#         cargo run -- run conformance/probes/augmented_lambda_value.mojo
def main():
    var total = 0
    total += (lambda -> Int: 1)()
    print(total)
