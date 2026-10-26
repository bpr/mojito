# A capture-list closure's value, and a capturing lambda's, is its closure's
# storage struct: it binds to a local, copies when its captures are
# copyable, moves out of its declaration, stores in a generic container, and
# passes through a `F: def(...)` binder, whose calls borrow it.
struct Tracked(ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        print("copy", copy.n)
        self.n = copy.n

    def __deinit__(deinit self):
        print("del", self.n)


struct Token(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("token drop", self.id)


def apply[F: def(Int) -> Int](f: F, a: Int) -> Int:
    return f(a)


def twice[F: def() -> Int](f: F) -> Int:
    return f() + f()


def bound_and_passed(a: Int) -> Int:
    var y = 10
    var z = 20

    def inner(x: Int) {var y, imm z} -> Int:
        return x + y + z

    var g = inner
    return apply(inner, a) + g(1)


def stored() -> Int:
    var y = 10

    def a() {var y} -> Int:
        return y

    var g = a
    var first = g()
    g = a
    var o = Optional(a)
    var l = [a, a]
    return first + g() + o.value()() + l[1]()


def lambda_locals(y: Int) -> Int:
    var g = lambda {var y} -> Int: y * 2
    var h = lambda -> Int: y + 1
    var k = lambda (x: Int) {var y} -> Int: x + y
    var m = k
    return g() + h() + k(1) + m(2)


def mutated() -> Int:
    var y = 1

    def inner(x: Int) {mut y} -> Int:
        y += x
        return y

    var a = apply(inner, 1)
    var b = apply(inner, 2)
    return a * 100 + b * 10 + y


def transferred() -> Int:
    var y = String("abc")

    def inner(x: Int) {var y} -> Int:
        return x + y.byte_length()

    var g = inner
    var h = g^
    return h(1) + inner(2)


def copied() -> Int:
    var t = Tracked(5)

    def inner() {var t} -> Int:
        return t.n

    print("decl")
    var g = inner
    print("bound")
    var r = g() + inner()
    print("called")
    var h = g
    print("copied")
    return r + h()


def main():
    print(bound_and_passed(2))
    print(stored())
    print(lambda_locals(5))
    print(mutated())
    print(transferred())
    print(copied())
    var token = Token(7)

    def peek() {var token^} -> Int:
        return token.id

    print(twice(peek))
    var g = peek^
    print(g())
    print("before scope end")
