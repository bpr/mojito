# expect: type mismatch for variable 'w': expected SIMD[dt, 4], found SIMD[dt, width]
# A symbolic width is its own type: `SIMD[dt, width]` is not `SIMD[dt, 4]`
# even inside the `width == 4` arm (a guard narrows nothing), and `v.join(v)`
# is `SIMD[dt, 2 * width]`, not the receiver's type.
def narrowed[dt: DType, width: Int](v: SIMD[dt, width]):
    comptime if width == 4:
        var w: SIMD[dt, 4] = v


def joined[dt: DType, width: Int](v: SIMD[dt, width]) -> SIMD[dt, width]:
    comptime if width == 4:
        return v.join(v)
    return v


def main():
    pass
