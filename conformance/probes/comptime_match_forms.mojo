# PROBE: `comptime match` over a type parameter with `== Int`, over a value with literals, a tuple binding, and `1 | 3 as k if k > 2`.
#
# The pin runs it and prints:
#   integer string other
#   three
#   m 4
#   guarded 3
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects `comptime match` until R1139 lands.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run comptime_match_forms.mojo
#         cargo run -- run conformance/probes/comptime_match_forms.mojo
def kind[T: AnyType]() -> String:
    comptime match T:
        case == Int:
            return "integer"
        case == String:
            return "string"
        case _:
            return "other"


def main():
    print(kind[Int](), kind[String](), kind[Bool]())
    comptime n = 3
    comptime match n:
        case 1:
            print("one")
        case 3:
            print("three")
        case _:
            print("many")
    comptime match (n, n + 1):
        case (3, m):
            print("m", m)
        case _:
            print("other")
    comptime match n:
        case 1 | 3 as k if k > 2:
            print("guarded", k)
        case _:
            print("no")
