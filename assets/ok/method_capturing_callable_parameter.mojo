# A method whose compile-time callable parameter binds a capturing
# `@parameter` closure. The method's instance folds the closure's body and
# takes its environment by reference, as a free function's does
# (`lambda_hof.mojo`). The pin prints `15` then `115`.
struct Runner(Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def apply[
        origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int
    ](self, value: Int) -> Int:
        return f(value) + self.base


def main():
    var factor = 3

    @parameter
    def scale(x: Int) -> Int:
        return x * factor

    print(scale(5))
    var runner = Runner(100)
    print(runner.apply[scale](5))
