# `DType` values across the compile-time boundary: a `DType` parameter default
# and annotation, a compile-time call returning a `DType` keying `SIMD[c, 2]`
# and `[dt: DType]` specializations (whose `comptime if` queries the dtype),
# `Optional`/tuple elements and tuple membership, reassignment, and hashing as
# the dtype's `UInt8` code.
def pick(flag: Bool) -> DType:
    if flag:
        return DType.int8
    return DType.uint64

def f(d: DType = DType.int) -> DType:
    return d

def width[dt: DType]() -> Int:
    comptime if dt.is_integral() and dt.is_signed():
        return 1
    return 2

def main():
    var x: DType = DType.float64
    print(f(), f(x))
    comptime c = pick(True)
    var v = SIMD[c, 2](5)
    print(v, width[c](), width[DType.uint64]())
    var o: Optional[DType] = DType.bool
    print(o.value())
    print(DType.int8 in (DType.int, DType.int8))
    var t = (DType.int, 3)
    print(t[0])
    var a = DType.int
    var b = a
    a = DType.uint8
    print(a, b)
    print(hash(DType.int8) == hash(UInt8(135)))
