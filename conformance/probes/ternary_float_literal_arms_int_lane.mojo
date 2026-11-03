# PROBE: a conditional expression of two float literals into an `Int32`.
#
# **Differs.** The pin converts each arm through `SIMD`'s `@implicit
# __init__(FloatLiteral)` and fails its "the SIMD type must be floating
# point" assert. Mojito joins the arms to the value-less `FloatLiteral`
# (`Float64`) and reports "expected Int32, found Float64". Filed as R1845.
# When Mojito matches, promote this file to `assets/type_error/`.
comptime A = 0.5


def main():
    var c = True
    var x: Int32 = A if c else 0.25
    print(x)
