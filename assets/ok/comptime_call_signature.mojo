# A signature or a struct field type that applies a module function to a
# compile-time parameter: `h(n)` there is the function applied to what its
# arguments denote, the same value wherever it is spelled. A body that
# spells the call again names it, a caller that binds the parameter spells
# it by the argument (`h(2)` for `h(n)` at `n = 2`), and a call infers the
# parameter from the argument. The elaborator runs the function per
# instance.

def h(n: Int) -> Int:
    return n * 2

def add(a: Int, b: Int) -> Int:
    return a + b

def big(n: Int) -> Bool:
    return n > 2

def pick(flag: Bool, n: Int) -> Int:
    if flag:
        return n
    return n * 4

def flag[b: Bool]():
    print(b)

struct Flag[b: Bool]:
    var x: Int

    def __init__(out self):
        self.x = 0

    def show(self):
        flag[Self.b]()

struct Buf[n: Int]:
    var total: Int

    def __init__(out self):
        self.total = Self.n

def make[n: Int]() -> SIMD[DType.int32, h(n)]:
    return SIMD[DType.int32, h(n)](7)

def nested[n: Int]() -> SIMD[DType.int32, h(h(n))]:
    var v: SIMD[DType.int32, h(h(n))] = 1
    return v

def arith[n: Int](v: SIMD[DType.int32, h(n) + n * 2]) -> SIMD[DType.int32, add(n, h(n + 1))]:
    return SIMD[DType.int32, add(n, h(n + 1))](5)

def flagged[n: Int]() -> Flag[big(n)]:
    return Flag[big(n)]()

def picked[n: Int, b: Bool]() -> SIMD[DType.int32, pick(b, n)]:
    return SIMD[DType.int32, pick(b, n)](9)

def total[n: Int](v: SIMD[DType.int32, h(n)]) -> Int:
    return Int(v.reduce_add()) + n

def both[a: Int, b: Int](v: SIMD[DType.int32, add(h(a), b)]) -> Int:
    return a * 10 + b

def size[n: Int](b: Buf[h(n)]) -> Int:
    return n

def user[m: Int]():
    var x: SIMD[DType.int32, h(m)] = make[m]()
    print(x)
    print(total[m](x))
    print(total(x))

struct Wrap[n: Int]:
    var v: SIMD[DType.int32, h(Self.n)]

    def __init__(out self):
        self.v = SIMD[DType.int32, h(Self.n)](3)

    def get(self) -> SIMD[DType.int32, h(Self.n)]:
        return self.v

    def widen[m: Int](self) -> SIMD[DType.int32, add(h(Self.n), h(m))]:
        return SIMD[DType.int32, add(h(Self.n), h(m))](1)

    def set(mut self, v: SIMD[DType.int32, h(Self.n)]):
        self.v = v

def unwrap[m: Int](w: Wrap[m]) -> SIMD[DType.int32, h(m)]:
    var x: SIMD[DType.int32, h(m)] = w.v
    return x

def closed() -> SIMD[DType.int32, h(2)]:
    return SIMD[DType.int32, h(2)](4)

# A field type may call a function declared below its struct.
struct Late[n: Int]:
    var v: SIMD[DType.int32, later(Self.n)]

    def __init__(out self):
        self.v = 6

def later(n: Int) -> Int:
    return n + n

def main():
    var v = make[2]()
    print(v)
    print(total[2](v))
    print(total(v))
    print(make[4]())
    var e: SIMD[DType.int32, h(2)] = make[2]()
    print(e)
    print(nested[1]())
    print(arith[2](SIMD[DType.int32, h(2) + 4](1)))
    flagged[1]().show()
    flagged[3]().show()
    print(picked[2, True]())
    print(picked[2, False]())
    print(both(SIMD[DType.int32, add(h(1), 2)](1)))
    print(size(Buf[h(3)]()))
    user[2]()
    var w = Wrap[2]()
    print(w.v)
    print(w.get())
    print(w.widen[2]())
    w.set(SIMD[DType.int32, h(2)](8))
    print(unwrap(w))
    print(unwrap[4](Wrap[4]()))
    var c: SIMD[DType.int32, h(2)] = closed()
    print(c)
    print(total[2](c))
    print(Late[2]().v)
