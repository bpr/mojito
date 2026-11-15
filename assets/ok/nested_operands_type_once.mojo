# Nested operands type once: each field projection, call, and tuple or list
# display types its operands a single time, as upstream's parser emits each
# operand once, so twenty levels of a generic struct built one per line, a
# nest of generic calls, a field chain, and nested displays check in time
# linear in their depth.

struct W[T: Copyable & Deinitable](Copyable):
    var x: Self.T

    def __init__(out self, x: Self.T):
        self.x = x.copy()


def wrap[T: Copyable & Deinitable](x: T) -> W[T]:
    return W[T](x.copy())


def main():
    var a0 = 7
    var a1 = W[Int](a0.copy())
    var a2 = W[W[Int]](a1.copy())
    var a3 = W[W[W[Int]]](a2.copy())
    var a4 = W[W[W[W[Int]]]](a3.copy())
    var a5 = W[W[W[W[W[Int]]]]](a4.copy())
    var a6 = W[W[W[W[W[W[Int]]]]]](a5.copy())
    var a7 = W[W[W[W[W[W[W[Int]]]]]]](a6.copy())
    var a8 = W[W[W[W[W[W[W[W[Int]]]]]]]](a7.copy())
    var a9 = W[W[W[W[W[W[W[W[W[Int]]]]]]]]](a8.copy())
    var a10 = W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]](a9.copy())
    var a11 = W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]](a10.copy())
    var a12 = W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]](a11.copy())
    var a13 = W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]](a12.copy())
    var a14 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]](a13.copy())
    var a15 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]]](a14.copy())
    var a16 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]]]](a15.copy())
    var a17 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]]]]](a16.copy())
    var a18 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]]]]]](a17.copy())
    var a19 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]]]]]]](a18.copy())
    var a20 = W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[W[Int]]]]]]]]]]]]]]]]]]]](a19.copy())
    print(a20.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x)
    var w = wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(wrap(8))))))))))))))))))))
    print(w.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x)
    var t = ((((((((((((((((((((9, 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1), 1)
    print(t[0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0])
    var l = [[[[[[[[[[[[[[[[[[[[10]]]]]]]]]]]]]]]]]]]]
    print(l[0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0][0])
