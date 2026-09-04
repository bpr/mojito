# atol/atof with Python's literal rules: whitespace and sign, `_`
# separators, base prefixes (base 0 detects them), the Int range, and the
# decimal/exponent float core with inf/nan; `Int(s)`/`Float64(s)` route
# through the raising `__int__`/`__float__`.
def main() raises:
    print(atol("123"), atol("  -42  "), atol("+7"), atol("1_000"))
    print(atol("0x1f", 16), atol("0x1f", 0), atol("0b101", 0), atol("0o17", 0), atol("ff", 16), atol("z", 36))
    print(atol("9223372036854775807"), atol("-9223372036854775808"), atol("0"), atol("-0"))
    print(atof("3.5"), atof("1e3"), atof("-.5"), atof("  2.25  "), atof("7"), atof("1.5e-2"))
    print(atof("inf"), atof("-inf"), atof("nan") != atof("nan"), atof("1E2"))
    print(Int(String("42")), Float64(String("2.5")))
    var text = String(" 77 ")
    print(Int(text) + 1, Float64(String("0.25")) * 4.0)
