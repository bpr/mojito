# A method keyed on its own compile-time value parameter specializes per call
# on a struct keyed on a value (`Width[n: Int]`) too: each call mints a clone
# of the method on the struct's specialization, whose signature may spell
# the struct's value (`SIMD[dt, Self.n]`), whether the lane is applied
# explicitly or read off the argument. The body may read `Self.n` and
# construct at its own lane, and an own `Int` binder specializes the same way.
# requires: discovery
struct Width[n: Int]:
    def __init__(out self):
        pass

    def rep[dt: DType](self, a: SIMD[dt, Self.n]) -> SIMD[dt, Self.n]:
        return a + a

    def bumped[dt: DType](self, a: SIMD[dt, Self.n]) -> SIMD[dt, Self.n]:
        return a + SIMD[dt, Self.n](1)

    def sized[dt: DType](self, a: SIMD[dt, Self.n]) -> Int:
        return Self.n + len(a)

    def widen[w: Int](self) -> SIMD[DType.int32, w]:
        return SIMD[DType.int32, w](Self.n)


struct Fixed[n: Int]:
    def __init__(out self):
        pass

    def rep[dt: DType](self, a: SIMD[dt, 4]) -> SIMD[dt, 4]:
        return a + a

    def keep(self, a: SIMD[DType.int8, Self.n]) -> SIMD[DType.int8, Self.n]:
        return a


def main():
    var w = Width[4]()
    print(w.rep[DType.int16](SIMD[DType.int16, 4](1, 2, 3, 4)))
    print(w.rep(SIMD[DType.int16, 4](5, 6, 7, 8)))
    print(w.bumped(SIMD[DType.float32, 4](1.5)))
    print(w.sized(SIMD[DType.uint8, 4](1)))
    print(w.widen[2]())
    var v = Width[2]()
    print(v.bumped(SIMD[DType.int64, 2](7)))
    var f = Fixed[4]()
    print(f.rep[DType.int16](SIMD[DType.int16, 4](1, 2, 3, 4)))
    print(f.keep(SIMD[DType.int8, 4](1)))
