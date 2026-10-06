# A type or a parameter argument that computes an `Int` or a `Bool` some way
# the parameter domain does not express: a call, a comparison, a boolean
# operator, or a conditional over a generic body's parameters, the loop
# index, a local `comptime` display binding, or a local `comptime` value.
# The check names a function for the expression, the template carries its
# application, and the elaborator runs it per instance. Two spellings of one
# expression are one type.

struct Buf[n: Int]:
    var total: Int

    def __init__(out self):
        self.total = Self.n

def g[e: Int]():
    print(e)

def flag[b: Bool]():
    print(b)

def h(x: Int) -> Int:
    return x * 2

def is_even(x: Int) -> Bool:
    return x % 2 == 0

def show[w: SIMDLength](x: SIMD[DType.int32, w]):
    print(x)

def plain[n: Int]():
    g[h(n)]()
    var a: SIMD[DType.int32, h(n)] = SIMD[DType.int32, h(n)](1)
    print(a)
    comptime e = h(n)
    g[e]()
    var c: SIMD[DType.int32, e] = SIMD[DType.int32, h(n)](2)
    show(c)
    flag[n > 2]()
    g[h(h(n)) - 3 * n]()

def displayed[n: Int]():
    comptime vals = [n, n * 2]
    g[h(vals[0])]()
    flag[vals[0] > 2]()
    g[min(vals[0], vals[1])]()
    g[h(vals[0]) + h(vals[1])]()
    g[max(len(vals), vals[1])]()
    flag[vals[0] == 2 and vals[1] == 4]()
    flag[not (vals[0] > 2)]()
    var v: SIMD[DType.int32, h(vals[0]) - vals[0]] = 5
    print(v)
    comptime for i in range(len(vals)):
        g[h(vals[i]) + i]()
        g[h(i)]()
        flag[is_even(vals[i] + i)]()
        comptime inner = [i, i + n]
        g[max(inner[0], inner[1])]()
    comptime b = n > 2
    flag[b]()
    comptime if b:
        print("b")
    var x: Buf[h(n)] = Buf[h(n)]()
    print(x.total)
    var y = Buf[h(vals[1])]()
    print(y.total)
    comptime e = h(vals[0])
    var c: SIMD[DType.int32, e] = SIMD[DType.int32, h(vals[0])](2)
    print(c)
    comptime t = (vals[0], vals[1])
    g[t[0] + t[1]]()
    g[h(t[1])]()
    g[n if b else h(n)]()

def iterated[n: Int]():
    comptime vals = [n, n * 2]
    comptime for x in vals:
        print(x)
    g[h(vals[0])]()
    comptime e = h(vals[1])
    g[e]()

struct Holder[T: AnyType, n: Int]:
    def __init__(out self):
        pass

    def widths[m: Int](self):
        comptime vals = [Self.n, m, Self.n + m]
        g[h(vals[2])]()
        g[h(Self.n) + h(m)]()
        var v = SIMD[DType.int32, h(vals[0])](7)
        print(v)
        comptime e = h(vals[1])
        g[e]()
        flag[Self.n > m]()

    def own(self):
        g[h(Self.n)]()
        var v: SIMD[DType.int32, h(Self.n)] = 3
        print(v)

def width[T: AnyType]() -> Int:
    return 8

def twice[k: Int]() -> Int:
    return k * 2

def typed[T: AnyType, n: Int]():
    g[width[T]() + n]()
    var a: SIMD[DType.int32, twice[n]()] = 1
    comptime for i in range(1):
        var b: SIMD[DType.int32, twice[n]()] = a
        print(b)

def closed():
    g[h(3)]()
    var a: SIMD[DType.int32, h(2)] = SIMD[DType.int32, h(2)](1)
    print(a)
    var b = Buf[h(3)]()
    print(b.total)
    flag[h(1) > 1]()
    comptime k = h(4)
    g[k]()

def main():
    plain[2]()
    plain[4]()
    displayed[2]()
    displayed[4]()
    iterated[2]()
    Holder[Int, 2]().widths[4]()
    Holder[Bool, 4]().widths[1]()
    Holder[Int, 2]().own()
    typed[Int, 2]()
    typed[Bool, 4]()
    closed()
