# A nested `def` keys its instances on the enclosing parameters it reads,
# and its enclosing function reads through it only those, as upstream's
# `RemoveUnusedParams` treats a lifted nested function: `ignores[1]` and
# `ignores[2]` share one instance of each body, at any depth, while a
# nested body that reads `n`, directly or through its own nested body, or
# a capture of `n` keeps one per value. A default reading `n` is the
# call's, not the body's: `defaulted[n]`'s `inner` has one instance.
def ignores[n: Int](x: Int) -> Int:
    def mid(y: Int) -> Int:
        def leaf(z: Int) -> Int:
            return z * 2

        return leaf(y) + 1

    return mid(x)


def reads[n: Int](x: Int) -> Int:
    def mid(y: Int) -> Int:
        def leaf(z: Int) -> Int:
            return z * n

        return leaf(y) + 1

    return mid(x)


def passes[n: Int](x: Int) -> Int:
    var total = x

    def add(y: Int) {total} -> Int:
        return total + y

    return add(n)


def as_value[n: Int](x: Int) -> Int:
    def inc(y: Int) -> Int:
        return y + 1

    var f = inc
    return f(x)


def defaulted[n: Int](k: Int) -> Int:
    var base = k

    def inner(x: Int, y: Int = n * 3) {imm base} -> Int:
        return base + x + y

    return inner(1) + inner(1, 2)


def typed[T: Copyable, n: Int](v: T) -> T:
    def keep(w: T) -> T:
        return w.copy()

    return keep(v)


@fieldwise_init
struct Counter[k: Int](Copyable):
    var v: Int

    def get[m: Int](self) -> Int:
        def bump(y: Int) -> Int:
            return y + 10

        return bump(self.v)


def main():
    print(ignores[1](3), ignores[2](4))
    print(reads[2](3), reads[3](4))
    print(passes[1](3), passes[2](3))
    print(as_value[1](3), as_value[2](4))
    print(defaulted[2](10), defaulted[3](10))
    print(typed[Int, 1](5), typed[Int, 2](6), typed[String, 1]("a"))
    print(Counter[1](1).get[1](), Counter[1](2).get[2]())
