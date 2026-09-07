# atof is upstream's Clinger fast path plus Eisel-Lemire over the generated
# power-of-five table: correctly rounded at the halfway cases and the
# normal/subnormal boundaries, `+` prefix and `f`/`F` suffix stripped,
# `nan` printed as upstream prints it, and upstream's limits and texts
# (24 digits at most; a significand beyond UInt64 is too large).
def show(s: String):
    try:
        print(atof(s))
    except e:
        print("error:", e)

def main():
    show("9007199254740993")
    show("9007199254740993e-1")
    show("1e23")
    show("8.98846567431158e307")
    show("2.2250738585072011e-308")
    show("4.9e-324")
    show("2.4703282292062327e-324")
    show("2.4703282292062328e-324")
    show("1e-400")
    show("1e400")
    show("1.7976931348623157e308")
    show("1.7976931348623159e308")
    show("123456789012345678e-5")
    show("1.5f")
    show("+2.5")
    show("  -0.1  ")
    show("-0")
    show("nan")
    show("-inf")
    show("Infinity")
    show("0.3")
    show("0.000001")
    show("3.14159265358979323846")
    show("1234567890123456789012345")
    show("18446744073709551616")
    show("1_0.5")
    show("abc")
    show("1e")
