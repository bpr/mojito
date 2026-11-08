# A compile-time call of a `SIMD` receiver's non-builtin method, in a
# signature, a type argument, or a `comptime` binding, is the method's
# application, which the elaborator runs.


struct T[n: Int32]:
    def __init__(out self):
        pass


struct L[n: Int]:
    def __init__(out self):
        pass


def summed[v: SIMD[DType.int32, 4]](var t: T[v.reduce_add()]):
    print("summed", v[0], t.n)


def largest[v: SIMD[DType.int32, 4]](var t: T[v.reduce_max()]):
    print("largest", v[3], t.n)


def lanes[v: SIMD[DType.int32, 4]](var t: L[v.__len__()]):
    print("lanes", v[1], t.n)


def widened[w: SIMD[DType.int32, 1]](var t: L[w.__int__()]):
    print("widened", w, t.n)


def in_body[v: SIMD[DType.int32, 4]]():
    var t = T[v.reduce_add()]()
    print("in_body", t.n)


def main():
    comptime v = SIMD[DType.int32, 4](1, 2, 3, 4)
    summed[v](T[v.reduce_add()]())
    largest[v](T[v.reduce_max()]())
    lanes[v](L[v.__len__()]())
    comptime w = Int32(7)
    widened[w](L[w.__int__()]())
    in_body[v]()
    comptime r: Int32 = v.reduce_add()
    print(r)
    var u = T[v.reduce_min()]()
    print(u.n)
