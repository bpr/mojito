# expect: no overload matches the supplied arguments
# Parentheses around an `assert`'s operands make one tuple condition, which
# matches no `debug_assert` overload, as at the pin.

def main():
    var n = 2
    assert(n == 2, "x")
    print("ok")
