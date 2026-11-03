# A method whose closure-trait binder takes a capture-list closure as a
# run-time argument, as a free function's does (`lambda_hof.mojo`). The pin
# prints `15` then `115`.
struct Runner(Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def apply[F: def(x: Int) -> Int](self, f: F, value: Int) -> Int:
        return f(value) + self.base


def main():
    var factor = 3

    def scale(x: Int) {imm factor} -> Int:
        return x * factor

    print(scale(5))
    var runner = Runner(100)
    print(runner.apply(scale, 5))
