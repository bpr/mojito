# A per-instantiation method clone inherits its checked template's facts when
# its body calls a method with explicit compile-time arguments
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `parameterized_calls`). The receiver is a non-generic struct, so the callee
# and the per-call clone its arguments request are the same under every
# instance: a value argument on a field of `self`, its result bound to a
# `var` local, a closed type argument, and a discarded call on a `mut`
# receiver derive for an `Int` and a `String` instance. A type argument
# naming the struct's own parameter (`show[Self.T]`) keeps the clone check.
struct Scaler(Copyable, Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def scaled[k: Int](self, x: Int) -> Int:
        return self.base + x * k

    def bump[step: Int](mut self):
        self.base += step

    def show[U: Writable & Copyable](self, value: U) -> String:
        return String(self.base) + ":" + String(value)


struct Holder[T: ImplicitlyCopyable & Deinitable & Writable](Movable):
    var item: Self.T
    var scaler: Scaler

    def __init__(out self, var item: Self.T, base: Int):
        self.item = item^
        self.scaler = Scaler(base)

    def triple(self, x: Int) -> Int:
        return self.scaler.scaled[3](x)

    def bound_double(self, x: Int) -> Int:
        var doubled = self.scaler.scaled[2](x)
        return doubled + 1

    def closed_show(self, x: Int) -> String:
        return self.scaler.show[Int](x)

    def own_show(self) -> String:
        return self.scaler.show[Self.T](self.item)

    def advance(mut self, x: Int) -> Int:
        self.scaler.bump[5]()
        return self.scaler.scaled[1](x)


def main():
    var a = Holder[Int](1, 10)
    var b = Holder[String](String("s"), 20)
    print(a.triple(2), b.triple(4))
    print(a.bound_double(1), b.bound_double(2))
    print(a.closed_show(7), b.closed_show(8))
    print(a.own_show(), b.own_show())
    print(a.advance(1), b.advance(2))
