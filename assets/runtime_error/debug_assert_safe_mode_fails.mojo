# A failing `assert_mode="safe"` assertion is enabled under the default
# `ASSERT` mode, so it reports its message and traps, as at the pin.
# expect: abort: safe boom 3


def main():
    var n = 2
    debug_assert[assert_mode="safe"](n == 3, "safe boom ", n + 1)
    print("unreachable")
