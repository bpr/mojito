# A pointer offset is the call of upstream's declared `unsafe_offset` (and
# its deprecated `__add__`), whose result a generic parameter received by
# address takes as its temporary's address. An offset of a pointer to one
# checked value runs too. Upstream prints `8`, `0`, `8`, and `4`.
from std.memory.alloc import unsafe_alloc


def address[T: Intable](v: T) -> Int:
    return Int(v)


def main():
    var buf = unsafe_alloc[Int](2)
    print(Int(buf + 1) - Int(buf))
    var q = buf.unsafe_offset(1)
    print(address(q + 0) - address(q))
    print(address(buf.unsafe_offset(1)) - Int(buf))
    var x = 4
    print(Pointer(to=x).unsafe_offset(0)[])
    buf.unsafe_free()
