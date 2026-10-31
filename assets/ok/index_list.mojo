# `std.utils.IndexList`, upstream's `StaticTuple`-backed index vector, with
# its constructors, element access, arithmetic, all-elements comparisons,
# shape helpers, `Index`, and `product`.
from std.utils import Index, IndexList, product


def main():
    var a = IndexList[3](1, 2, 3)
    var b = IndexList[3](4, 5, 6)
    print(a, len(a), a[0], a.get[2]())
    print(IndexList[2]())
    print(IndexList[4](fill=7))
    var l: IndexList[2] = [10, 11]
    print(l)
    a[1] = 20
    print(a)
    print(a + b, b - a, a * b, b // a, a.remu(b))
    print(a == a, a == b, a < b, a <= b, a > b, a >= b)
    print(a.reverse(), a.flattened_length(), a.get_row_major_strides())
    var c = a.cast[DType.int32]()
    print(c, c.canonicalize())
    print(a.as_tuple()[2])
    print(Index(1, 2, 3), Index[dtype=DType.int32](4, 5))
    print(product(b), product(b, 2), product(b, 1, 3))
    print(IndexList[1](5))
    print(hash(a) == hash(a))
    print(IndexList[3, element_type=DType.int32].get_type_name())
