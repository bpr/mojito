# A pointer formed over a place has an integer address, as upstream's one
# `!kgen.pointer` value does: `Int(p)`, `UInt(p)`, `p.__int__()`, an
# `Intable` generic, and one reached through an `@always_inline` generic
# forward all read it, two pointers to one place agree, and pointers to
# distinct places differ. Upstream prints `True` eight times.
@fieldwise_init
struct Pair(Copyable):
    var a: Int
    var b: Int


def addr[T: Intable](v: T) -> Int:
    return Int(v)


def inner[T: Intable](value: T) -> Int:
    return Int(value)


@always_inline
def outer[T: Intable](value: T) -> Int:
    return inner(value)


def main():
    var x = 5
    var p = Pointer(to=x)
    print(Int(p) != 0)
    print(p.__int__() != 0)
    print(UInt(p) != 0)
    print(addr(p) != 0)
    print(outer(Pointer(to=x)) == Int(p))
    var y = 6
    print(Int(Pointer(to=y)) != Int(p))
    var s = Pair(1, 2)
    print(Int(Pointer(to=s.a)) != Int(Pointer(to=s.b)))
    print(Int(Pointer(to=s.a)) == Int(Pointer(to=s.a)))
