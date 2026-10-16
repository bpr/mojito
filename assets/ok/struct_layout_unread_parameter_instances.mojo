# A struct type reads only the parameters its lowered layout reads, as
# upstream's `RemoveUnusedParams` walks the structural types `LowerLIT`
# left: `fb[n](b: Box[n])` has one instance for `Box[1]` and `Box[2]`,
# since `Box` holds an `Int` whatever `n` is, as do a function that only
# forwards `n` to it, one over `Holder[Box[n]]`, one over a struct whose
# pointer field points to `T`, and one over a struct holding a `List[T]`,
# whose storage is a pointer, and one that calls a method on its argument
# that reads no struct parameter. A struct holding a `T` or a
# `SIMD[int64, n]` keeps one instance per argument, as does a function
# returning `Arr[n]` by value, and one whose dropped argument's destructor
# prints `Self.n`.
from std.memory.alloc import unsafe_alloc


struct Box[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct Arr[n: Int](Copyable, Movable):
    var data: SIMD[DType.int64, Self.n]

    def __init__(out self):
        self.data = SIMD[DType.int64, Self.n](7)


struct Holder[T: Copyable & Deinitable](Copyable, Movable):
    var k: Int
    var x: Self.T

    def __init__(out self, k: Int, var x: Self.T):
        self.k = k
        self.x = x^


struct PtrBox[T: Copyable & Deinitable](Copyable, Movable):
    var k: Int
    var p: UnsafePointer[Self.T, MutUntrackedOrigin]

    def __init__(out self, k: Int):
        self.k = k
        self.p = unsafe_alloc[Self.T](1)


struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]
    var tag: Int

    def __init__(out self, tag: Int):
        self.items = List[Self.T]()
        self.tag = tag


struct Pair[a: Int, b: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct NBox[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __del__(deinit self):
        print("del", Self.n, self.v)

    def get(self) -> Int:
        return self.v


@no_inline
def fb[n: Int](b: Box[n]) -> Int:
    return b.v + 1


@no_inline
def fwd[n: Int](b: Box[n]) -> Int:
    return fb(b) + 1


@no_inline
def fh[T: Copyable & Deinitable](h: Holder[T]) -> Int:
    return h.k


@no_inline
def fhb[n: Int](h: Holder[Box[n]]) -> Int:
    return h.k


@no_inline
def fpb[T: Copyable & Deinitable](p: PtrBox[T]) -> Int:
    return p.k


@no_inline
def tag_of[T: Copyable & Deinitable](b: Bag[T]) -> Int:
    return b.tag


@no_inline
def boxed_tag[n: Int](b: Bag[Box[n]]) -> Int:
    return b.tag + 1


@no_inline
def fs[n: Int](a: Arr[n]) -> Int:
    return Int(a.data[0])


@no_inline
def fp[a: Int, b: Int](p: Pair[a, b]) -> Int:
    return a + p.v


@no_inline
def fv[n: Int](var p: Pair[n, 3]) -> Int:
    return p.v


@no_inline
def fm[n: Int](mut p: Pair[n, 3]):
    p.v += 1


@no_inline
def pass_[n: Int](var a: Arr[n]) -> Arr[n]:
    return a^


@no_inline
def fn_[n: Int](var b: NBox[n]) -> Int:
    return b.v


@no_inline
def fg[n: Int](b: NBox[n]) -> Int:
    return b.get()


def main():
    print(fb(Box[1](1)), fb(Box[2](2)))
    print(fwd(Box[1](1)), fwd(Box[2](2)))
    print(fh(Holder[Int](3, 4)), fh(Holder[Float64](5, 6.0)))
    print(fhb(Holder[Box[1]](3, Box[1](1))), fhb(Holder[Box[2]](4, Box[2](2))))
    print(fpb(PtrBox[Int](8)), fpb(PtrBox[Float64](9)))
    print(tag_of(Bag[Int](10)), tag_of(Bag[String](11)))
    print(boxed_tag(Bag[Box[1]](12)), boxed_tag(Bag[Box[2]](13)))
    print(fs(Arr[1]()), fs(Arr[2]()))
    print(fp(Pair[1, 2](10)), fp(Pair[1, 5](20)))
    var q1 = Pair[7, 3](1)
    var q2 = Pair[8, 3](2)
    fm(q1)
    fm(q2)
    print(fv(q1^), fv(q2^))
    var c = pass_(Arr[1]())
    var d = pass_(Arr[2]())
    print(c.data, d.data)
    print(fn_(NBox[1](1)), fn_(NBox[2](2)))
    print(fg(NBox[1](3)), fg(NBox[2](4)))
