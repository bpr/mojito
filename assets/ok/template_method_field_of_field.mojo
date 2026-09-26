# A per-instantiation method clone inherits its checked template's facts when
# its body reads or writes a field of a field of `self` (`self.scaler.base`,
# `self.inner.count += v`) or reads a field of a struct parameter, `var` or
# read (`s.base`) (`docs/notes/instantiation-from-template.md`, class
# MethodBody). A field has its declared type under its base's recorded
# arguments, so an `Int` and a `String` instance reach the same path, whether
# the intermediate field is closed (`Scaler`) or built over the struct's
# parameter (`Inner[Self.T]`).
struct Scaler(Copyable, Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base


struct Inner[T: ImplicitlyCopyable & Deinitable](Copyable, Movable):
    var item: Self.T
    var count: Int

    def __init__(out self, item: Self.T, count: Int):
        self.item = item
        self.count = count


struct Holder[T: ImplicitlyCopyable & Deinitable & Writable](Movable):
    var item: Self.T
    var scaler: Scaler
    var inner: Inner[Self.T]

    def __init__(out self, var item: Self.T, base: Int):
        self.inner = Inner[Self.T](item, base * 2)
        self.item = item^
        self.scaler = Scaler(base)

    def base(self) -> Int:
        return self.scaler.base

    def offset(self, x: Int) -> Int:
        return self.scaler.base + x

    def count(self) -> Int:
        return self.inner.count

    def reset(mut self, v: Int):
        self.scaler.base = v
        self.inner.count += v

    def item_copy(self) -> Self.T:
        return self.inner.item

    def take(self, var s: Scaler) -> Int:
        return s.base + self.scaler.base

    def peek(self, s: Scaler) -> Int:
        return s.base * 2


def main():
    var a = Holder[Int](1, 10)
    var b = Holder[String](String("s"), 20)
    print(a.base(), b.base())
    print(a.offset(2), b.offset(3))
    print(a.count(), b.count())
    a.reset(5)
    b.reset(6)
    print(a.base(), b.base(), a.count(), b.count())
    print(a.item_copy(), b.item_copy())
    print(a.take(Scaler(3)), b.take(Scaler(4)))
    print(a.peek(Scaler(3)), b.peek(Scaler(4)))
