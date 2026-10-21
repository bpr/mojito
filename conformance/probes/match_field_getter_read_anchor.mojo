# PROBE: a getter called on a field of a place `match` subject that no case binds by reference (`W(l=[a, b])`, `W(o=.Some(x))`).
#
# The pin runs it and prints:
#   field 1 2
#   some 4
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects it with "access to 'w.l' conflicts with live reference
# '$match<id>'" until R1240 lands; with a `ref` binding in the case (a
# mutable anchor) the same reads run.
#
# Run:    mojo run match_field_getter_read_anchor.mojo
#         cargo run -- run conformance/probes/match_field_getter_read_anchor.mojo
@fieldwise_init
struct W(Copyable):
    var l: List[Int]
    var o: Optional[Int]


def main():
    var w = W([1, 2], Optional(4))
    match w:
        case W(l=[a, b]):
            print("field", a, b)
        case _:
            print("other")
    match w:
        case W(o=.Some(x)):
            print("some", x)
        case _:
            print("other")
