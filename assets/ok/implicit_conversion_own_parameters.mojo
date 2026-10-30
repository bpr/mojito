# A converting constructor with compile-time parameters of its own
# (`Variant.__init__[T: Movable](out self, var value: T)`) solves them from
# the converted value, at a binding, an assignment, an argument, a return,
# and a method argument. At equal cost a constructor without parameters of
# its own beats a generic one.
from std.utils import Variant

comptime V = Variant[Int, String]

def show(v: V):
    if v.isa[Int]():
        print("int", v[Int])
    else:
        print("str", v[String])

def make() -> V:
    return String("returned")

struct Shown:
    var text: String

    @implicit
    def __init__[T: Writable](out self, value: T):
        self.text = String(value)

    def __init__(out self, value: Int):
        self.text = "int"

struct Tagged[E: Copyable & Writable & Deinitable](Writable):
    var e: Self.E
    var tag: String

    @implicit
    def __init__[T: Intable](out self, x: T) where Self.E == Int:
        self.e = rebind[Self.E](Int(x)).copy()
        self.tag = "generic"

    @implicit
    def __init__(out self, e: Self.E):
        self.e = e.copy()
        self.tag = "plain"

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.tag, ":", self.e)

def shown(s: Shown):
    print(s.text)

def tagged(t: Tagged[Int]):
    print(t)

def main():
    var v: V = String("hi")
    show(v)
    v = String("ho")
    show(v)
    show(3)
    show(String("arg"))
    show(make())
    var vs = List[V]()
    vs.append(String("elem"))
    show(vs[0])
    var s = Shown(1)
    print(s.text)
    s = String("hey")
    print(s.text)
    shown(2.5)
    tagged(7)
    tagged(Int8(3))
