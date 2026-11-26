# Upstream's `Pointer(unsafe_from_address=)` names the bytes at an address,
# whatever the storage there holds: an address inside a heap element, or a
# pointee of another layout than the storage's own, reads and writes the
# storage's bytes, as an address inside a local struct does. Reading the
# address of each such pointer gives back the integer it was made from.

from std.memory import Pointer
from std.memory.alloc import unsafe_alloc


@fieldwise_init
struct Pair(Copyable):
    var a: Int
    var b: Int


def heap_bytes_of_words():
    var p = unsafe_alloc[Int](2)
    p[unsafe_offset=0] = 0x0102
    p[unsafe_offset=1] = 5
    var a = Int(p)
    var q = Pointer[Byte, MutAnyOrigin](unsafe_from_address=a + 1)
    print(q[], Int(q) == a + 1)
    q[] = 3
    print(p[unsafe_offset=0])
    var r = Pointer[Byte, MutAnyOrigin](unsafe_from_address=a + 8)
    print(r[])
    var u = Pointer[UInt32, MutAnyOrigin](unsafe_from_address=a + 8)
    print(u[])
    p.unsafe_free()


def heap_words_of_bytes():
    var p = unsafe_alloc[Byte](16)
    for i in range(16):
        p[unsafe_offset=i] = Byte(i)
    var q = Pointer[Int, MutAnyOrigin](unsafe_from_address=Int(p) + 8)
    print(q[])
    var r = Pointer[Int, MutAnyOrigin](unsafe_from_address=Int(p) + 1)
    print(r[])
    r[] = -1
    print(p[unsafe_offset=0], p[unsafe_offset=1], p[unsafe_offset=8], p[unsafe_offset=9])
    p.unsafe_free()


def place_field():
    var s = Pair(1, 2)
    var bp = Pointer(to=s).unsafe_bitcast[Byte]()
    var q = Pointer[Int, origin_of(s)](unsafe_from_address=Int(bp) + 8)
    print(q[], Int(q) == Int(bp) + 8)
    q[] = 7
    print(s.a, s.b)


def heap_struct_fields():
    var p = unsafe_alloc[Pair](2)
    p.unsafe_write(Pair(1, 2))
    p.unsafe_offset(1).unsafe_write(Pair(3, 4))
    var a = Int(p)
    print(Pointer[Int, MutAnyOrigin](unsafe_from_address=a + 8)[])
    print(Pointer[Int, MutAnyOrigin](unsafe_from_address=a + 16)[])
    print(Pointer[Pair, MutAnyOrigin](unsafe_from_address=a + 16)[].b)
    p.unsafe_free()


def main():
    heap_bytes_of_words()
    heap_words_of_bytes()
    place_field()
    heap_struct_fields()
