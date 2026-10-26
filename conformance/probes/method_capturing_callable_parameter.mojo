# Defect probe (Mojo 1.2.0.dev2026092105): a method whose compile-time
# callable parameter binds a capturing closure. The pin runs it and prints
# `15` then `115`. The free-function form of the same shape is
# `assets/ok/lambda_hof.mojo`, and the closure alone runs in Mojito too;
# the method call stops the elaborator with "unsupported binding call to
# `main$scale` during monomorphization" (roadmap R172).
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
