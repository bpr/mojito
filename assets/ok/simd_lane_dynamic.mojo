# Lane reads and writes with run-time indexes: loop counters, arithmetic
# indexes, and an `Indexer` value normalized once; the reduction of a
# `DType.int` vector is the native `Int` a scalar entry can return.
@fieldwise_init
struct Offset(Indexer):
    var value: Int
    def __mlir_index__(self) -> __mlir_type.index:
        return self.value

def compute() -> Int:
    var v = SIMD[DType.int, 8](0)
    for i in range(8):
        v[i] = i * i
    for i in range(1, 8):
        v[i] += v[i - 1]
    var acc: Int = 0
    for i in range(8):
        acc += v[7 - i] * (i + 1)
    v[Offset(3)] = acc
    return v.reduce_add() + v[Offset(3)]

def main():
    print(compute())
    var f = SIMD[DType.float32, 8](0.0)
    for i in range(8):
        f[i] = Float32(i) / 4.0
    var m = SIMD[DType.uint8, 16](0)
    var j = 0
    while j < 16:
        m[j] = m[15 - j] + UInt8(j * 17)
        j += 1
    print(f, m, f[7], m[15])
    var w = SIMD[DType.int16, 4](1, 2, 3, 4)
    var idx = 3
    while idx >= 0:
        w[idx] = w[idx] * 1000
        idx -= 1
    print(w)
