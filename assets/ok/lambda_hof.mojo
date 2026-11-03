# Lambdas as higher-order-function arguments: a thin lambda binds to a plain
# `def(...) thin` contract, and a capture-list closure to a closure-trait
# binder `F: def(...)` taken as a run-time argument.
def transform(f: def(x: Int) thin -> Int, value: Int) -> Int:
    return f(value)

def observe[F: def(x: Int) -> Int](f: F, value: Int) -> Int:
    return f(value)

def main():
    print(transform(lambda (x: Int) -> Int: x * 2, 4))
    print(transform(lambda (x: Int) {} -> Int: x + 1, 4))
    var factor = 3

    def scale(x: Int) {imm factor} -> Int:
        return x * factor

    print(observe(scale, 5))
    factor = 2
    print(observe(scale, 5))
