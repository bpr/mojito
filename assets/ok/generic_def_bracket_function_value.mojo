# A bracket over a generic `def` read as a value (`h[4]`, `inner[1]`) is
# that specialization, a function value: bound to a `var`, rebound, passed
# to a callable parameter, or bracketed inside a generic enclosing `def`,
# each call runs the instance, as at the pin, where a nested `def`'s
# bracket must name its instance's closure.
def h[k: Int](t: Int) -> Int:
    return t + k


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def nested(x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return y + m

    var f = inner[1]
    return f(x) + apply(inner[5], x)


def defaulted(x: Int) -> Int:
    def scale[m: Int = 3](y: Int) -> Int:
        return y * m

    var f = scale[4]
    return f(x)


def enclosing[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return y + m + n

    var f = inner[1]
    return f(x)


def shown[T: Copyable & Writable](x: T):
    def show[count: Int](y: T):
        for _ in range(count):
            print(y)

    var f = show[2]
    f(x)


def main():
    var g = h[4]
    print(g(1))
    g = h[4]
    print(g(2), apply(h[10], 1))
    print(nested(2))
    print(defaulted(2))
    print(enclosing[100](2))
    shown(7)
    shown(String("s"))

    def local[n: Int]() -> Int:
        return n * 3

    var l = local[4]
    print(l())
