# A method's receiver reads only the struct parameters its struct's layout
# reads, as upstream's `RemoveUnusedParams` walks `self`'s lowered
# structural type: `Box[1](1).get()` and `Box[2](2).get()` share one
# instance of a `get` that ignores `n`, as do `take(var self)`,
# `bump(mut self)`, and `Wrap.get`, whose `DBox[n]` field lays out an `Int`.
# A method keeps one instance per value where it reads `Self.n` and where
# its struct's layout reads `n` (`Arr[n]` holds a `SIMD[int64, n]`), as on
# the pin. The pin also keeps one `twice`, which calls `self.get()`, one
# `Wrap.take`, whose drop of `self` runs `DBox`'s destructor, and one
# destructor and constructor; Mojito still keeps those one per value.
struct Box[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def get(self) -> Int:
        return self.v

    def take(var self) -> Int:
        return self.v

    def bump(mut self):
        self.v += 1

    def reads(self) -> Int:
        return self.v + Self.n

    def twice(self) -> Int:
        return self.get() + self.get()


struct Arr[n: Int](Copyable, Movable):
    var data: SIMD[DType.int64, Self.n]

    def __init__(out self, v: Int):
        self.data = SIMD[DType.int64, Self.n](v)

    def first(self) -> Int:
        return Int(self.data[0])


struct DBox[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct Wrap[n: Int](Movable):
    var d: DBox[Self.n]

    def __init__(out self, var d: DBox[Self.n]):
        self.d = d^

    def get(self) -> Int:
        return self.d.v

    def take(var self) -> Int:
        return self.d.v * 10


def main():
    print(Box[1](1).get(), Box[2](2).get())
    print(Box[1](3).take(), Box[2](4).take())
    var a = Box[1](5)
    var b = Box[2](6)
    a.bump()
    b.bump()
    print(a.v, b.v)
    print(a.reads(), b.reads())
    print(a.twice(), b.twice())
    print(Arr[1](7).first(), Arr[2](8).first())
    var w1 = Wrap[1](DBox[1](1))
    var w2 = Wrap[2](DBox[2](2))
    print(w1.get(), w2.get())
    print(w1^.take())
    print(w2^.take())
