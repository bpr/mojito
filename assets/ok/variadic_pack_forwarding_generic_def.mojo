# A variadic struct applied over an enclosing generic's own parameters:
# a comptime-class def forwards its pack (`Variant[*Ts]`), a bound-generic
# def spells the pack partially (`Variant[T, String]`) and is called by
# inference, and a variadic struct stores the forwarded pack in a field,
# constructed from a generic body over its own parameter too.
# The retained template bodies check against the template's shell; each
# clone's signature requests the concrete specialization.
# requires: discovery
from std.utils import Variant

def first_variant[*Ts: Movable]() -> Variant[*Ts]:
    return Variant[*Ts](3)

def wrap[T: Movable](var x: T) -> Variant[T, String]:
    return Variant[T, String](x^)

struct Outer[*Ts: Movable & Deinitable]:
    var v: Variant[*Self.Ts]

    def __init__(out self, var v: Variant[*Self.Ts]):
        self.v = v^

def wrap_outer[T: Movable & Deinitable](var x: T) -> Outer[T, Int]:
    return Outer[T, Int](Variant[T, Int](x^))

def main():
    var value = first_variant[Int, String]()
    print(value.isa[Int](), value.isa[String]())
    var wrapped = wrap(3)
    print(wrapped.isa[Int](), wrapped.isa[String]())
    var flag = wrap(True)
    print(flag.isa[Bool](), flag.isa[String]())
    var o = Outer[Int, Bool](Variant[Int, Bool](True))
    print(o.v.isa[Int](), o.v.isa[Bool]())
    var nested = wrap_outer(String("hi"))
    print(nested.v.isa[String](), nested.v.isa[Int]())
