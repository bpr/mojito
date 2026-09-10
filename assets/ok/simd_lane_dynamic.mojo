# Lane reads and writes with run-time indexes: loop counters, arithmetic
# indexes, and an index carried in a local; the reduction of a `DType.int`
# vector is the native `Int` a scalar entry can return. The widths a scalar
# entry cannot return live in `simd_lane_dynamic_widths`; `Indexer`
# normalization lives in `indexer_normalization` — upstream's
# `SIMD.__getitem__` takes a plain `Int`, so a lane cannot spell one here.
def compute() -> Int:
    var v = SIMD[DType.int, 8](0)
    for i in range(8):
        v[i] = i * i
    for i in range(1, 8):
        v[i] += v[i - 1]
    var acc: Int = 0
    for i in range(8):
        acc += v[7 - i] * (i + 1)
    var slot = 1 + 2
    v[slot] = acc
    return v.reduce_add() + v[slot]

def main():
    print(compute())
