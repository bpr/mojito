# A capturing `@parameter` closure bound to a compile-time callable parameter
# and forwarded to another binding: each instance folds the closure's body and
# takes its environment by reference, so the callee it forwards to calls the
# body directly and sees writes made between calls.
def inner[
    origins: OriginSet, //, f: def(Int) capturing[origins] -> Int
](x: Int) -> Int:
    return f(x)


def outer[
    origins: OriginSet, //, f: def(Int) capturing[origins] -> Int
](x: Int) -> Int:
    return inner[f](x) + 1


struct Runner(Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def apply[
        origins: OriginSet, //, f: def(Int) capturing[origins] -> Int
    ](self, value: Int) -> Int:
        return outer[f](value) + self.base


def main():
    var k = 10

    @parameter
    def add(x: Int) -> Int:
        return x + k

    print(outer[add](5))
    k = 20
    print(outer[add](5))
    var runner = Runner(100)
    print(runner.apply[add](1))
