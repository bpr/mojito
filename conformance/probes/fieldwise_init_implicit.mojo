# PROBE: `@fieldwise_init("implicit")` makes the synthesized initializer
# `@implicit`.
#
# The pin runs it and prints:
#   3
#
# Mojito rejects it: `@fieldwise_init("implicit")` is unsupported (R1163).
@fieldwise_init("implicit")
struct S:
    var x: Int

def main():
    var s: S = 3
    print(s.x)
