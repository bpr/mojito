# PROBE: a negative literal index into an `Array`.
#
# The pin rejects it at compile time ("constraint failed: negative indexing
# is not supported"); Mojito compiles it and traps at run time with
# "Pointer access out of bounds".
#
# Observed 2026-10-08 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run array_negative_literal_index.mojo
#         cargo run -- run conformance/probes/array_negative_literal_index.mojo
def main():
    var a = [1, 2, 3]
    print(a[-1])
