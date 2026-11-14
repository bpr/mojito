# `T(copy=x)` over a memory-only `T` no pointer may bind passes its source as
# the copy initializer's `read copy: Self` operand, a reference to the
# caller's storage; each instance reads it as its binding's copy does: a
# fieldwise struct's value copy, a struct's own copy initializer, a
# register-passable struct's value, or a built-in's value read, in a plain
# body, an `@always_inline` one, and a `comptime for` body.

trait Valued(Copyable, Deinitable):
    def value(self) -> Int: ...


@fieldwise_init
struct Box(Valued):
    var v: Int
    var s: String
    def value(self) -> Int:
        return self.v


@fieldwise_init
struct Counted(Valued):
    var v: Int
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def value(self) -> Int:
        return self.v


@fieldwise_init
struct Plain(Valued, TrivialRegisterPassable):
    var v: Int
    def value(self) -> Int:
        return self.v


def make[T: Valued](x: T) -> T:
    return T(copy=x)


def twice[T: Valued](x: T) -> Int:
    var y = T(copy=x)
    return y.value() + x.value()


def builtin[T: Copyable & Comparable & Deinitable](x: T) -> T:
    return T(copy=x)


@always_inline
def inlined[T: Copyable & Comparable & Deinitable](x: T) -> T:
    return T(copy=x)


def unrolled[T: Copyable & Comparable & Deinitable](x: T) -> Int:
    var n = 0
    comptime for i in range(3):
        var y = T(copy=x)
        if y == x:
            n += i
    return n


def main():
    var b = make(Box(4, "yo"))
    print(b.v, b.s)
    print(make(Counted(1)).v)
    print(make(Plain(7)).v)
    print(twice(Box(2, "a")), twice(Counted(1)), twice(Plain(3)))
    print(builtin(3), builtin(String("hi")), builtin(2.5), builtin(True))
    print(inlined(4), inlined(String("yo")))
    print(unrolled(5), unrolled(String("z")))
