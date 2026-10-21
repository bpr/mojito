# PROBE: a comprehension clause's sequence target (`[f for [f, *_] in rows]`) and place target (`[q[0] for q[0] in xs]`) destructure each element, as a `for` statement's do.
#
# The pin runs it and prints:
#   2 1 4
#   2 7 8 8
#
# Mojito: rejects both, "a sequence or place target in a comprehension
# clause", because a comprehension lowers in MIR as an expression and its
# clause cannot run the statements those targets desugar to (R1227).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run comprehension_sequence_target.mojo
#         cargo run -- run conformance/probes/comprehension_sequence_target.mojo
def main() raises:
    var rows: List[List[Int]] = [[1, 2, 3], [4, 5]]
    var firsts = [f for [f, *_] in rows]
    print(len(firsts), firsts[0], firsts[1])
    var q: List[Int] = [0]
    var seen = [q[0] for q[0] in [7, 8]]
    print(len(seen), seen[0], seen[1], q[0])
