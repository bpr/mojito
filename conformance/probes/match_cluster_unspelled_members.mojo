# PROBE: a cluster keys an `Equal` member with no single-token spelling by
# the empty spelling, so implicitly concatenated string literals share one
# group, tested by the first member's literal.
#
# The pin runs it and prints nothing: `"a" "b"` and `"c" "d"` both key as
# `""`, sort first, and form one group whose test is `== "a" "b"`; `"cd"`
# fails it and `"zz"`, and no case runs. (upstream's
# `StringLiteralNode::getLiteralSpelling` returns no spelling for a
# concatenation, and `SpellingOrEnumCase::getLeadingTest` keys it as `""`,
# its assertion compiled out.) It also warns `unreachable code after match
# case that always completes` at the third case's body, the group's second
# residual, which R1242's dead-region pass would report.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito follows the pin and prints nothing; before R1140 it printed `cd`.
# docs/notes/pattern-matching-evidence.md records the quirk. Re-probe after
# a re-pin: if upstream keys a concatenation by its value, follow it.
#
# Run:    mojo run match_cluster_unspelled_members.mojo
#         cargo run -- run conformance/probes/match_cluster_unspelled_members.mojo
def main():
    match String("cd"):
        case "zz":
            print("zz")
        case "a" "b":
            print("ab")
        case "c" "d":
            print("cd")
