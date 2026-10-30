# `size_of`'s second parameter is a `CompilationTarget`, which a type does
# not bind, so neither overload matches `size_of[Int, Bool]()`, as at the pin.
# expect: no overload matches the supplied arguments
from std.sys import size_of


def main():
    print(size_of[Int, Bool]())
