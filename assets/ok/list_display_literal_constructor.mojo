# A list display at a type other than `List` or `Array` is upstream's
# `T(a, b, __list_literal__=None)`: the construction of its contextual type
# through that type's `__list_literal__` initializer, `SIMD`'s declared
# element list or a struct's own. It holds as a binding, an argument, a
# return value, a list element, a `comptime` binding, and a parameter
# argument, where a brace initializer list is likewise its construction.


struct Count(ImplicitlyCopyable, Writable):
    var n: Int

    def __init__(out self, *elems: Int, __list_literal__: NoneType = None):
        self.n = len(elems)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Count(", self.n, ")")


def total(v: SIMD[DType.int32, 4]) -> Int32:
    return v.reduce_add()


def halves() -> SIMD[DType.float64, 2]:
    return [0.5, 4]


def size(c: Count) -> Int:
    return c.n


def lanes[v: SIMD[DType.int32, 2]]() -> Int32:
    return v.reduce_add()


def counted[c: Count]() -> Int:
    return c.n


def main():
    var v: SIMD[DType.int32, 4] = [1, 2, 3, 4]
    print(v)
    var w: SIMD[DType.float32, 2] = [1.5, 2]
    print(w)
    var c: Count = [1, 2, 3]
    print(c)
    var e: Count = []
    print(e)
    print(total([1, 2, 3, 4]))
    print(halves())
    print(size([7, 8]))
    var l: List[SIMD[DType.int32, 2]] = [[1, 2], [3, 4]]
    print(l[1])
    comptime k: Count = [1, 2, 3, 4, 5]
    print(k)
    print(lanes[[5, 6]]())
    print(lanes[{7, 8}]())
    print(counted[[1, 2]]())
    print(counted[{1, 2, 3}]())
