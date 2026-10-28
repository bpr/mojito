# A bare `IntLiteral` parameter (upstream's `IntLiteral[_]`) takes a literal
# argument, and beside an `Int` overload a literal selects it while an `Int`
# value selects the `Int` one, through an implicit conversion or a call.
struct L(Copyable, Movable):
    var v: Int

    @implicit
    def __init__(out self, value: IntLiteral, /):
        self.v = 100 + Int(value)

    @implicit
    def __init__(out self, value: Int, /):
        self.v = value

    def take(self, x: IntLiteral) -> Int:
        return self.v + Int(x) * 1000

    def take(self, x: Int) -> Int:
        return self.v + x


def twice(x: IntLiteral) -> Int:
    return Int(x) * 2

def f(x: IntLiteral) -> Int:
    var y: Int = x
    return y + twice(x) + 10

def f(x: Int) -> Int:
    return x

def main():
    var n = 4
    var a: L = 3
    var b: L = n
    print(a.v, b.v)
    print(L(3).v, L(n).v)
    print(f(5), f(n), f(2 + 3), f(-1))
    print(a.take(2), a.take(n))
