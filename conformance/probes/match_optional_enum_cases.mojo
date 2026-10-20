# PROBE: `.Some(x)` / `.None` / `Optional.Some(0)` / `Optional.Some(var x)` / `.Some(ref s)` cases over `Optional`, in `match`, `if case`, OR, and guarded forms.
#
# The pin runs it and prints:
#   some 3 none
#   zero some+1 5 none
#   hi!
#   hi!
#   any
#   short hi!
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects enum case patterns until R1132/R1133 land.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run match_optional_enum_cases.mojo
#         cargo run -- run conformance/probes/match_optional_enum_cases.mojo
def describe(o: Optional[Int]) -> String:
    match o:
        case .Some(x):
            return String("some ", x)
        case .None:
            return "none"


def describe2(o: Optional[Int]) -> String:
    match o:
        case Optional.Some(0):
            return "zero"
        case Optional.Some(var x):
            x += 1
            return String("some+1 ", x)
        case Optional.None:
            return "none"


def main():
    print(describe(Optional[Int](3)), describe(None))
    print(describe2(Optional[Int](0)), describe2(Optional[Int](4)), describe2(None))
    var o: Optional[String] = String("hi")
    match o:
        case .Some(ref s):
            s += "!"
        case .None:
            pass
    print(o.value())
    if case .Some(v) = o:
        print(v)
    match o:
        case .Some(_) | .None:
            print("any")
    match o:
        case .Some(x) if x.byte_length() > 10:
            print("long")
        case .Some(x):
            print("short", x)
        case .None:
            print("none")
