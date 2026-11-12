# A local `comptime` tuple binding is its literal constructor's
# application, as the pin's alias is its value: an element read through
# the binding, a second binding of the same display, an alias of it, the
# inline display, or the binding inside a `comptime for` is one parameter
# expression, and the binding binds a tuple parameter.


def h(x: Int) -> Int:
    return x + 1


def through_loop[n: Int]():
    comptime t = (n, n * 2)
    var a: SIMD[DType.int32, h(t[0])] = SIMD[DType.int32, h(t[0])](1)
    comptime for i in range(2):
        comptime u = (n, n * 2)
        var b: SIMD[DType.int32, h(u[0])] = a
        var c: SIMD[DType.int32, h(t[0])] = b
        print(c)


def two_bindings[n: Int]():
    comptime t = (n, n * 2)
    comptime u = (n, n * 2)
    var a: SIMD[DType.int32, h(t[0])] = SIMD[DType.int32, h(u[0])](2)
    print(a)


def alias_and_inline[n: Int, m: Int]():
    comptime t = (n, n * 2)
    comptime t2 = t
    var a: SIMD[DType.int32, h(t[0])] = SIMD[DType.int32, h(t2[0])](3)
    var b: SIMD[DType.int32, h(t2[0])] = SIMD[DType.int32, h((n, n * 2)[0])](
        4
    )
    print(a + b)


def sum_of[t: Tuple[Int, Int]]():
    print(t[0] + t[1])


def main():
    through_loop[3]()
    two_bindings[3]()
    alias_and_inline[3, 5]()
    comptime x = (3, 4)
    sum_of[x]()
    print(x[0])
