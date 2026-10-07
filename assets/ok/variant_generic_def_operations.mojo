# A generic `def` calls `Variant` operations over its own binder: the
# template's `_get_type_index[T, *Ts]()` closes at each instance.
from std.utils import Variant

def pick[T: Copyable & Writable](v: Variant[Int, String]) -> Bool:
    return v.isa[T]()

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    print(pick[Int](v), pick[String](v))
    v.set[String](String("x"))
    print(v[String])
    print(v == Variant[Int, String](String("x")))
