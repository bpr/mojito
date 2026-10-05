# A vector argument solves both a `DType` binder and a width binder of SIMD's
# own width type (`SIMDLength`), a native scalar as a width-one vector, and
# upstream's `SIMD[_, _]` parameter spelling stands for exactly those two
# infer-only binders, on a `def`, a method, or a trait requirement. A width
# binder forwards the caller's own, open one.
def bits(value: SIMD[_, _]) -> UInt64:
    return value.to_bits().cast[DType.uint64]().reduce_add()


def lanes[dt: DType, w: SIMDLength](v: SIMD[dt, w]) -> Int:
    return w


def forward[n: Int](v: SIMD[DType.int8, n]) -> Int:
    return lanes(v)


trait Sink:
    def put(mut self, v: SIMD[_, _]):
        ...


struct Tally(Sink):
    var count: Int

    def __init__(out self):
        self.count = 0

    def add[dt: DType, w: SIMDLength, //](mut self, v: SIMD[dt, w]):
        self.count += w

    def add_any(mut self, v: SIMD[_, _]):
        self.count += v.length

    def put(mut self, v: SIMD[_, _]):
        self.count += 100 * v.length


def feed[S: Sink](mut sink: S):
    sink.put(SIMD[DType.int8, 4](1))


def main():
    print(bits(Int(40)), bits(UInt8(3)), bits(SIMD[DType.int16, 2](1, -1)))
    print(lanes(SIMD[DType.int32, 4](1, 2, 3, 4)), lanes(Float64(2.5)), lanes(Int32(3)))
    print(forward[8](SIMD[DType.int8, 8](1)))
    var t = Tally()
    t.add(SIMD[DType.uint8, 8](1))
    t.add(Float32(1.0))
    t.add_any(SIMD[DType.int64, 2](1, 2))
    feed(t)
    print(t.count)
