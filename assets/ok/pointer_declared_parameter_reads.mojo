# A `Pointer`'s parameters, `comptime` members, and conformances read
# through its declaration in `std.memory.pointer`, as upstream's do.
from std.memory.alloc import unsafe_alloc


def show_int[T: Intable](t: T) -> Int:
    return Int(t)


def show_text[T: Writable](t: T) -> String:
    return String(t)


def main():
    var x = 5
    var p = Pointer(to=x)
    print(p.address_space)
    print(p.mut)
    var o = p.origin
    print(o.mut)
    comptime T = type_of(p).T
    var y: T = 3
    var z: p.T = 4
    print(y, z)
    print(String(p).startswith("0x"), Int(p) != 0)
    print(show_int(p) == Int(p), show_text(p).startswith("0x"))
    print(conforms_to(type_of(p), Writable), conforms_to(type_of(p), Intable))
    var q = unsafe_alloc[Int](1)
    q[] = 3
    print(String(q).startswith("0x"), q[])
    q.unsafe_free()
