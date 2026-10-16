# A constructor that reads no struct parameter is one instance for every
# struct instance: a direct construction, an implicit conversion, a
# converting default, and a `T()` through `Defaultable` all reach it.
struct Box[n: Int](Movable):
    var v: Int

    @no_inline
    def __init__(out self, v: Int):
        self.v = v


struct Tag[n: Int](ImplicitlyCopyable):
    var v: Int

    @implicit
    @no_inline
    def __init__(out self, v: Int):
        self.v = v


struct Z[n: Int](Defaultable, Movable):
    var v: Int

    @no_inline
    def __init__(out self):
        self.v = 42


def f(t: Tag[3] = 5):
    print(t.v)


def g(t: Tag[4]):
    print(t.v)


def make[T: Defaultable & Movable]() -> T:
    return T()


def count(var **kwargs: Int) -> Int:
    return len(kwargs)


def main():
    var a = Box[1](1)
    var b = Box[2](2)
    print(a.v, b.v)
    f()
    f(6)
    g(7)
    var t: Tag[5] = 8
    print(t.v)
    var z = make[Z[1]]()
    var y = make[Z[2]]()
    print(z.v, y.v)
    print(count(p=1, q=2))
