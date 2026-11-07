# Each `debug_assert` overload, at its default `assert_mode="none"`, is
# skipped under the default `ASSERT` mode (`safe`), and a passing
# `assert_mode="safe"` one runs to its end, as at the pin.


def main():
    var n = 2
    debug_assert(n == 3)
    debug_assert(n == 3, "literal")
    debug_assert(n == 3, "n is ", n)

    def check() {n} -> Bool:
        return n == 3

    debug_assert(check)
    debug_assert(check, "closure ", n)
    debug_assert[assert_mode="safe"](n == 2)
    debug_assert[assert_mode="safe"](n == 2, "fine")
    debug_assert[assert_mode="safe"](n == 2, "fine ", n)
    debug_assert[assert_mode="safe", _use_compiler_assume=True](n == 2)
    debug_assert[_use_compiler_assume=True](n == 2, "assumed")
    print("ok")
