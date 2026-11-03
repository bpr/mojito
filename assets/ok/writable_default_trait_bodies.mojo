# `Writable`'s `write_to` and `write_repr_to` are the trait's own default
# bodies over `_reflection_write_to`: a conformer declaring neither writes
# `Name(field=value, ...)`, each field through its own member, nested
# conformers included.


@fieldwise_init
struct Point(Copyable, Writable):
    var x: Int
    var y: String


@fieldwise_init
struct Outer(Writable):
    var p: Point
    var f: Float64


def main():
    var p = Point(1, "a")
    print(p)
    print(repr(p))
    print(Outer(p.copy(), 2.5))
    print(String(p))
