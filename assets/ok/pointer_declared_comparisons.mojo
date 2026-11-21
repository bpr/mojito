# `Pointer`'s comparisons, `-`, and `offset_from` run upstream's declared
# bodies, which compare and subtract the addresses `Int(self)` reads. A
# pointer of one origin selects the `rhs: Self` overload and one of another
# origin the `rhs: Pointer[Self.T, _]` overload, `Pointer` conforms to
# `Comparable`, so a generic bounded by it takes a pointer, and two pointers
# to one checked value compare equal. Upstream prints the same lines.
from std.memory.alloc import unsafe_alloc


def smaller[T: Comparable](a: T, b: T) -> Bool:
    return a < b


def main():
    var p = unsafe_alloc[Int](4)
    var q = p.unsafe_offset(2)
    print(p < q, p <= q, p > q, p >= q, p == q, p != q)
    print(q - p, p - q, q.offset_from(p))
    print(p.__lt__(p), p.__le__(p), p == p)
    print(smaller(p, q), smaller(q, p))
    var x = 4
    var r = Pointer(to=x)
    print(Pointer(to=x) == Pointer(to=x), r != r, r < r)
    print(r == r.as_imm(), r.as_imm() >= r)
    p.unsafe_free()
