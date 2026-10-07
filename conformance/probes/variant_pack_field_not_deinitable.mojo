# Probe: a struct whose pack is bounded `Copyable` only stores a
# `Variant[*Self.Ts]` field.
#
# The pin (2026-10-06) rejects the field: "field 'v' has non-'Deinitable'
# type 'Variant[*Ts.values]'", since `Variant` is `Deinitable` only where
# every alternative is. Mojito accepts it and prints `True False`.
# docs/roadmap.md R408.
from std.utils import Variant

struct Box[*Ts: Copyable](Copyable):
    var v: Variant[*Self.Ts]

    def __init__(out self, var x: Variant[*Self.Ts]):
        self.v = x^

def main():
    var b = Box[Int, String](Variant[Int, String](String("hi")))
    print(b.v.isa[String](), b.v.isa[Int]())
