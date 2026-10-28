# A compile-time value of the MLIR builtin `index`: a parameter of type
# `__mlir_type.index` binds one, `Int.__mlir_index__()` and
# `Int(mlir_value=i)` fold between it and an `Int` (distributing over `+`
# and `*`, a round trip cancelling), and a builtin callable over `index`
# folds, as at the pin.

comptime I = Int(5).__mlir_index__()


@fieldwise_init
struct S[n: __mlir_type.index](Copyable, Movable):
    @staticmethod
    def show():
        print(Int(mlir_value=Self.n))


@fieldwise_init
struct T[n: Int](Copyable, Movable):
    pass


struct D[n: __mlir_type.index = Int(7).__mlir_index__()]:
    @staticmethod
    def show():
        print(Int(mlir_value=Self.n))


struct Q[n: Int]:
    comptime idx = Self.n.__mlir_index__()

    @staticmethod
    def show():
        print(Int(mlir_value=Self.idx))


@always_inline("builtin")
def add(a: __mlir_type.index, b: __mlir_type.index) -> __mlir_type.index:
    return __mlir_op.`index.add`(a, b)


@always_inline("builtin")
def twice(n: __mlir_type.index) -> __mlir_type.index:
    return __mlir_op.`index.add`(n, n)


@always_inline("builtin")
def to_index(n: Int) -> __mlir_type.index:
    return __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.index](
        n._mlir_value
    )


@always_inline("builtin")
def from_index(i: __mlir_type.index) -> Int:
    return Int(
        mlir_value=__mlir_op.`pop.cast_from_builtin`[
            _type=__mlir_type.`!kgen.scalar<index>`
        ](i)
    )


def f[n: __mlir_type.index]():
    print(Int(mlir_value=n))


def offset[n: __mlir_type.index](x: Int) -> Int:
    return x + Int(mlir_value=n)


def copied[n: __mlir_type.index]():
    var r = n
    print(Int(mlir_value=r))


def eq[a: __mlir_type.index, b: __mlir_type.index]() -> Bool:
    return Int(mlir_value=a) == Int(mlir_value=b)


def h[k: Int, j: Int]():
    S[k.__mlir_index__()].show()
    var a: S[k.__mlir_index__()] = S[(k + 0).__mlir_index__()]()
    var b: T[Int(mlir_value=k.__mlir_index__())] = T[k]()
    var c: S[add(k.__mlir_index__(), j.__mlir_index__())] = S[
        add(j.__mlir_index__(), k.__mlir_index__())
    ]()
    var d: S[add(k.__mlir_index__(), j.__mlir_index__())] = S[
        (k + j).__mlir_index__()
    ]()
    var e: S[to_index(k)] = S[k.__mlir_index__()]()
    var f: T[from_index(k.__mlir_index__())] = T[k]()
    _ = a^
    _ = b^
    _ = c^
    _ = d^
    _ = e^
    _ = f^


def g[k: __mlir_type.index]():
    var e: S[Int(mlir_value=k).__mlir_index__()] = S[k]()
    _ = e^
    print(Int(mlir_value=k))


def main():
    S[Int(3).__mlir_index__()].show()
    f[Int(5).__mlir_index__()]()
    S[I].show()
    h[42, 1]()
    g[Int(8).__mlir_index__()]()
    print(eq[Int(1).__mlir_index__(), Int(1).__mlir_index__()]())
    comptime assert Int(mlir_value=Int(2).__mlir_index__()) == 2
    print(materialize[Int(mlir_value=Int(-7).__mlir_index__())]())
    S[twice(Int(3).__mlir_index__())].show()
    Q[11].show()
    D[].show()
    print(offset[Int(2).__mlir_index__()](40))
    copied[Int(3).__mlir_index__()]()
    var u: S[UInt(3).__mlir_index__()] = S[Int(3).__mlir_index__()]()
    _ = u^
