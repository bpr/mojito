# A run-time `==` between two compile-time `StaticString` values, two
# parameters or a module `comptime` and a parameter in either order,
# compares their text, as at the pin.

comptime MODE: StaticString = "safe"


def same[a: StaticString, b: StaticString]() -> Bool:
    return b == a


def is_mode[m: StaticString]() -> Bool:
    return MODE == m


def mode_is[m: StaticString]() -> Bool:
    return m == MODE


def main():
    print(same["safe", "safe"]())
    print(same["safe", "none"]())
    print(is_mode["safe"]())
    print(is_mode["all"]())
    print(mode_is["safe"]())
    print(mode_is["warn"]())
