@fieldwise_init
struct Inner(Copyable, Movable):
    var a: Int
    var b: Bool


@fieldwise_init
struct Outer(Copyable, Movable):
    var inner: Inner
    var scale: Float64

    def total(self) -> Float64:
        if self.inner.b:
            return Float64(self.inner.a) * self.scale
        return self.scale

    def grown(self, by: Float64) -> Outer:
        return Outer(Inner(self.inner.a + 1, self.inner.b), self.scale + by)


def consume(o: Outer) -> Int:
    return o.inner.a


def main():
    var o = Outer(Inner(6, True), 2.5)
    print(o.total(), o.inner.a, o.inner.b)
    var p = o
    p.inner.a = 41
    print(o.inner.a, p.inner.a)
    var g = o.grown(0.5)
    print(g.total(), g.inner.a)
    var i = 0
    var acc = 0
    while i < 4:
        var t = Inner(i, False)
        acc = acc + t.a
        i = i + 1
    print(acc, consume(g))
