# A capture-list closure bound to a closure-trait binder and forwarded to
# another binding, from a function and from a method: the callee it forwards
# to calls the closure's body and sees writes made between calls.
def inner[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def outer[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return inner(f, x) + 1


struct Runner(Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def apply[F: def(Int) -> Int](self, f: F, value: Int) -> Int:
        return outer(f, value) + self.base


def main():
    var k = 10

    def add(x: Int) {imm k} -> Int:
        return x + k

    print(outer(add, 5))
    k = 20
    print(outer(add, 5))
    var runner = Runner(100)
    print(runner.apply(add, 1))
