# A `DType`- or lane-keyed `def` is an ordinary generic `def`: its template
# serves every call, whatever its other binders are. A value binder a runtime
# parameter names outside a lane slot (`n` of `a: Box[n]`) is inferred from
# the argument's type, as are an erased origin beside the lane, a lane-keyed
# struct argument, and a lane width; a `thin` callable and a vector- or
# tuple-valued binder are bound from the call's brackets. A type or a pack
# binder beside an inferred value is served the same way.
struct Box[n: Int](Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct DBox[dt: DType](Copyable):
    var v: Scalar[Self.dt]

    def __init__(out self, v: Scalar[Self.dt]):
        self.v = v


def h[dt: DType, n: Int](a: Box[n], b: Scalar[dt]) -> Int:
    return n


def w[dt: DType, n: Int](a: Box[n], b: SIMD[dt, n]) -> Int:
    return n + Int(b.reduce_add())


def d[dt: DType](a: DBox[dt]) -> Scalar[dt]:
    return a.v + 1


def e[dt: DType, n: Int](a: Box[n], b: Scalar[dt]) -> Scalar[dt]:
    var s = SIMD[dt, n](b)
    return s.reduce_add()


def o2[
    dt: DType, mut: Bool, //, origin: Origin[mut=mut]
](ref[origin] x: Scalar[dt]) -> Scalar[dt]:
    return x + 1


def twice(x: Int) -> Int:
    return 2 * x


def c[dt: DType, f: def(Int) thin -> Int](x: Scalar[dt]) -> Int:
    return f(Int(x))


def m[dt: DType, v: SIMD[DType.int32, 2]](x: Scalar[dt]) -> Int32:
    return v.reduce_add() + Int32(Int(x))


def q[dt: DType, t: Tuple[Int, Int]](x: Scalar[dt]) -> Int:
    return t[0] + Int(x)


def b[T: Writable, n: Int](a: Box[n], x: T) -> Int:
    print(x)
    return n


def p[n: Int, *Ts: Writable](a: Box[n], *args: *Ts) -> Int:
    print(*args)
    return n


def main():
    print(h(Box[3](0), Int32(1)))
    print(h[DType.int8, 2](Box[2](0), Int8(1)))
    print(w(Box[2](0), SIMD[DType.int32, 2](5, 6)))
    print(d(DBox[DType.int16](Int16(7))))
    print(e(Box[4](0), Float32(1.5)))
    print(e[DType.int64, 2](Box[2](0), Int64(3)))
    var a = Int32(4)
    print(o2(a))
    print(c[DType.int8, twice](Int8(3)))
    print(m[DType.int8, SIMD[DType.int32, 2](1, 2)](Int8(3)))
    print(q[DType.int8, (5, 6)](Int8(3)))
    print(b(Box[2](0), "y"))
    print(p(Box[3](0), 1, "x"))
