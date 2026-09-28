# A module `def`'s reference result is read through wherever a value is
# wanted: bound by `var` (a copy), handed to `print`, used as an operand,
# assigned, copied through `.copy()`, and read inside a method that stores
# it. Each read at an argument's last use still sees the argument.
def pick[T: Copyable](ref a: T, ref b: T, first: Bool) -> ref[origin_of(a, b)] T:
    if first:
        return a
    return b


def echo[o: Origin](ref[o] x: String) -> ref[o] String:
    return x


struct Holder[T: ImplicitlyCopyable & Deinitable](Movable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def choose(mut self, a: Self.T, b: Self.T, first: Bool):
        var r = pick(a, b, first)
        self.value = r^

    def copied(mut self, a: Self.T, b: Self.T, first: Bool):
        self.value = pick(a, b, first).copy()


def main():
    var w = String("w")
    print(echo(w))
    var x = 1
    var y = 2
    var r = pick(x, y, False)
    r += 5
    print(r, y)
    print(pick(x, y, True) + 1)
    var s = String("s")
    var t = echo(s)
    print(t)
    var u = String("u")
    var v = String("")
    v = echo(u)
    print(v)
    var c = pick(String("c"), String("d"), True).copy()
    print(c)
    var h = Holder(0)
    h.choose(1, 2, True)
    print(h.value)
    h.copied(3, 4, False)
    print(h.value)
    var n = Holder(String("x"))
    n.choose(String("a"), String("b"), False)
    print(n.value)
