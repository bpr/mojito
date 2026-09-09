# Bool vectors are stored values too: in variables and struct fields, built
# from comparisons, combined with `& | ^ ~`, shuffled, selected over, and
# reduced with `reduce_and`/`reduce_or`.
@fieldwise_init
struct Flags:
    var mask: SIMD[DType.bool, 8]

def flip(m: SIMD[DType.bool, 8]) -> SIMD[DType.bool, 8]:
    return ~m

def main():
    var m = SIMD[DType.bool, 8](True, False, True, False, True, True, False, False)
    var n = SIMD[DType.bool, 8](True)
    print(m, n, ~m, m & n, m | ~n, m ^ n, m == n, m != n)
    var f = Flags(m)
    f.mask[1] = True
    f.mask[0] = False
    print(f.mask, flip(f.mask), f.mask[1], f.mask.reduce_and(), f.mask.reduce_or())
    var v = SIMD[DType.int16, 8](1, 2, 3, 4, 5, 6, 7, 8)
    var gt = v > 4
    print(gt, gt.shuffle[7, 6, 5, 4, 3, 2, 1, 0](), gt.shuffle[0, 7]())
    print(gt.select(v, -v), gt.select(v, 0), gt.select(100, v))
    print((gt & m).reduce_or(), (gt | m).reduce_and(), n.reduce_and(), (m ^ m).reduce_or())
    var b1 = SIMD[DType.bool, 2](False, True)
    print(b1, ~b1, b1.select(SIMD[DType.float64, 2](1.5, 2.5), 0.0))
