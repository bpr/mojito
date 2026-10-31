# A compile-time call of an ordinary method passes the struct value it is
# called on as its receiver: `self` and `var self` by value, `ref self`
# stored in compile-time memory, in a signature, in a struct method's
# return type, and at struct scope.


@fieldwise_init
struct L(ImplicitlyCopyable):
    var n: Int

    def ord(self) -> Int:
        return self.n - 1

    def get(ref self) -> Int:
        return self.n

    def get_at[o: Origin](ref[o] self) -> Int:
        return self.n * 3

    def owned(var self) -> Int:
        self.n += 2
        return self.n

    def scaled[k: Int](self) -> Int:
        return self.n * k


def f[n: Int]() -> SIMD[DType.int8, L(n).ord()]:
    return SIMD[DType.int8, L(n).ord()](1)


def g[n: Int]() -> SIMD[DType.int8, L(n).get() + L(n).get_at()]:
    return SIMD[DType.int8, L(n).get() + L(n).get_at()](2)


struct Tagged[l: L]:
    comptime m = Self.l.owned() + Self.l.scaled[3]()

    def __init__(out self):
        pass

    def make(self) -> SIMD[DType.int8, Self.l.get()]:
        return SIMD[DType.int8, Self.l.get()](1)


def main():
    print(f[3]())
    print(g[1]())
    var t = Tagged[L(1)]()
    print(t.make())
    print(Tagged[L(2)].m)
    comptime k = L(4).scaled[2]()
    print(k)
