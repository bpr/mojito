# `type_of(value)` is the value's type wherever a type is read: an
# annotation, a bracket argument, a `comptime` binding, and a type
# comparison. The operand is typed without being evaluated.
@fieldwise_init
struct P(Copyable, Writable):
    var a: Int

    def write_to(self, mut w: Some[Writer]):
        w.write("P(", self.a, ")")


def f[T: AnyType](x: Int) -> Int:
    return x


def g(x: Int, y: Float64):
    var a: type_of(y) = 1.5
    var b: List[type_of(x)] = [x]
    print(a, len(b))


def twice[T: Copyable & Writable & Deinitable](x: T):
    var y: type_of(x) = x.copy()
    comptime U = type_of(x)
    var z: U = y.copy()
    print(y, z, type_of(x) == T)


def show[W: Writer](mut writer: W):
    comptime WriterType = type_of(writer)
    var s: String = "hi"
    writer.write(s)


def main():
    var x = 3
    var y: type_of(x) = 4
    print(y)

    var p = P(1)
    var q: type_of(p) = P(2)
    var l = List[type_of(p)]()
    l.append(q.copy())
    print(q, len(l))

    g(1, 2.0)

    var s: String = "a"
    var t: type_of(s) = "b"
    var u: type_of(s + t) = "c"
    var pair = (1, "x")
    var other: type_of(pair) = (2, "y")
    print(t, u, other[0], other[1])

    twice(3)
    twice(String("s"))

    comptime T = type_of(x)
    var w: T = 5
    print(w, T == Int, type_of(x) == Int)
    print(f[type_of(x)](6))
    print(materialize[type_of(t) == String](), type_of(y) == Float64)

    var out = String()
    show(out)
    print(out)
