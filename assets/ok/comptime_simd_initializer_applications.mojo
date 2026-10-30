# A compile-time `SIMD` construction is its declared initializer: a builtin
# one (`from_int=`, the default, a splat) folds over `pop.simd.splat` and
# `pop.cast`, and any other (`fill=`) is an application the elaborator
# runs, in a parameter, a type, or a module constant.


struct Tag[v: SIMD[DType.int32, 2]](Defaultable):
    def __init__(out self):
        pass


comptime B = SIMD[DType.bool, 2](fill=True)
comptime Y = SIMD[DType.int32, 2](from_int=5)
comptime Z = SIMD[DType.int32, 2]()
comptime C = SIMD[DType.int32, 2](from_int=5) + SIMD[DType.int32, 2](from_int=1)


def mask[m: SIMD[DType.bool, 2]]():
    print(m)


def quad[v: SIMD[DType.int32, 4]]():
    print(v)


def pair[v: SIMD[DType.int32, 2]]():
    print(v)


def tagged(t: Tag[SIMD[DType.int32, 2](3)]):
    print("same type")


def splat_of[n: Int]():
    pair[SIMD[DType.int32, 2](n)]()
    var t: Tag[SIMD[DType.int32, 2](n)] = Tag[
        SIMD[DType.int32, 2](from_int=n)
    ]()
    _ = t^


def main():
    mask[SIMD[DType.bool, 2](fill=True)]()
    quad[SIMD[DType.int32, 4](from_int=6)]()
    quad[SIMD[DType.int32, 4]()]()
    print(B, Y, Z, C)
    var a = Y
    print(a)
    comptime if Y[0] == 5:
        print("five")
    tagged(Tag[SIMD[DType.int32, 2](from_int=3)]())
    splat_of[4]()
