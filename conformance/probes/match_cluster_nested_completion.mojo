# PROBE: a cluster's group whose residual cases form a nested `hlcf.match`,
# inside an enclosing `match` case, completes the enclosing case when a
# residual's body runs.
#
# The pin runs it and prints:
#   nested residual match then outer case
#     eq 1 1
#     guard a
#   B
# and then crashes (`execution crashed`): after the nested match upstream's
# `emitCases` emits `hlcf.unreachable` (the residual list concludes) or
# `hlcf.match.next` where the enclosing case should complete, so a residual
# body that runs falls into it. With `case == S(7).v` as the third case it
# prints `eq 1 7` before `eq 1 1`.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito completes the enclosing case and prints `B`, then the second and
# third matches' lines (`C 2`; `C 1` after `guard a`). docs/non-goals.md
# records the divergence; fixtures avoid the shape. Re-probe after a re-pin:
# when upstream emits `hlcf.match.complete` there, the shape may go into a
# fixture.
#
# Run:    mojo run match_cluster_nested_completion.mojo
#         cargo run -- run conformance/probes/match_cluster_nested_completion.mojo
@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __eq__(self, other: Int) -> Bool:
        print("  eq", self.v, other)
        return self.v == other


def g(tag: String) -> Bool:
    print("  guard", tag)
    return False


def main():
    print("nested residual match then outer case")
    match S(1):
        case 1 if g("a"):
            print("A")
        case 1:
            print("B")
        case var x:
            print("C", x.v)
    print("nested residual match, value 2")
    match S(2):
        case 1 if g("a"):
            print("A")
        case 1:
            print("B")
        case var x:
            print("C", x.v)
    print("guard fails all of group")
    match S(1):
        case 1 if g("a"):
            print("A")
        case 2:
            print("two")
        case var x:
            print("C", x.v)
