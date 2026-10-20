# PROBE: `@align(n)` raises a struct's alignment.
#
# The pin runs it and prints:
#   16 3
#
# Mojito rejects it: "decorator '@align' on a struct" is unsupported
# (R1161).
from std.sys import size_of

@align(16)
@fieldwise_init
struct A:
    var x: Int

def main():
    print(size_of[A](), A(3).x)
