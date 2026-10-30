# `size_of` without a type or a dtype matches neither of `std.sys.info`'s
# overloads, as at the pin.
# expect: no overload matches the supplied arguments
from std.sys import size_of


def main():
    print(size_of())
