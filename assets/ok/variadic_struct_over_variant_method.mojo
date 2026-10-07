# A variadic struct stores a `Variant` over its own pack and forwards a
# type-keyed query to it.
from std.utils import Variant

struct Box[*Ts: Copyable & Deinitable](Copyable):
    var v: Variant[*Self.Ts]

    def __init__[T: Copyable](out self, var x: T):
        self.v = Variant[*Self.Ts](x^)

    def has[T: Copyable](self) -> Bool:
        return self.v.isa[T]()

def main():
    var b = Box[Int, String](String("hi"))
    print(b.has[String](), b.has[Int]())
