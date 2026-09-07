# requires: discovery
# A `Tuple[...]` application resolves in any type-argument position: the
# nullary default construction over a nested Tuple element, a one-element
# nested Tuple, bare `Tuple(...)` calls nesting without an annotation, a
# `T: Defaultable` bound over the nested type, and a SIMD element that
# default-constructs to zero lanes.
def make[T: Defaultable]() -> T:
    return T()

def main():
    var t = Tuple[Int, Tuple[Int, Bool]]()
    print(t[0], t[1][0], t[1][1])
    var one = Tuple[Int, Tuple[Int]]()
    print(one[0], one[1][0])
    var x = Tuple(1, True)
    var u = Tuple(x, 2)
    print(u[0][0], u[0][1], u[1])
    var v = Tuple(Tuple(3, False), 4)
    print(v[0][0], v[0][1], v[1])
    var m = make[Tuple[Int, Tuple[Int, Bool]]]()
    print(m[0], m[1][0], m[1][1])
    var s = Tuple[SIMD[DType.int, 4], Int]()
    print(s[0], s[1])
