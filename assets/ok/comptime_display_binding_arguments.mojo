# A type or a parameter argument that reads a local `comptime` display
# binding over a generic body's parameters: an element by position or the
# length, alone or under integer arithmetic, and a local `comptime` value
# bound to one. The template serves each; the elaborator evaluates the
# display per instance.

struct Buf[n: Int]:
    var total: Int

    def __init__(out self):
        self.total = Self.n

def g[e: Int]():
    print(e)

def flag[b: Bool]():
    print(b)

def show[w: SIMDLength](x: SIMD[DType.int32, w]):
    print(x)

def plain[n: Int]():
    comptime L = [n, n * 2]
    var v: SIMD[DType.int32, L[0]] = SIMD[DType.int32, L[0]](1)
    print(v)
    comptime e = L[1]
    g[e]()
    g[L[0]]()
    g[len(L)]()

def looped[n: Int]():
    comptime vals = [n, n * 2]
    comptime for x in vals:
        print(x)
    var a: SIMD[DType.int32, vals[0]] = 1
    print(a)
    var w: SIMD[DType.int32, vals[0] + 0] = SIMD[DType.int32, vals[1 - 1]](3)
    print(w)
    var b = Buf[vals[1]]()
    print(b.total)
    show(a)
    comptime k = len(vals) + vals[0]
    g[k]()
    g[vals[1] - vals[0]]()
    comptime for i in range(len(vals)):
        g[vals[i] + i]()
        comptime inner = [i, i + n]
        g[inner[1]]()
    comptime flags = [n > 2, n > 3]
    flag[flags[0]]()

struct Holder[T: AnyType, n: Int]:
    def __init__(out self):
        pass

    def widths[m: Int](self):
        comptime vals = [Self.n, m, Self.n + m]
        g[vals[2]]()
        var v = SIMD[DType.int32, vals[0]](7)
        print(v)
        comptime e = vals[1]
        g[e]()

def main():
    plain[2]()
    plain[4]()
    looped[2]()
    looped[4]()
    Holder[Int, 2]().widths[4]()
    Holder[Bool, 4]().widths[1]()
