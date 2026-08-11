# Lambdas as higher-order-function arguments: a thin lambda binds to a plain
# `def(...)` contract and a capturing lambda to a `capturing[...]` contract.
def transform(f: def(x: Int) -> Int, value: Int) -> Int:
    return f(value)

def observe(f: def(x: Int) capturing[_] -> Int, value: Int) -> Int:
    return f(value)

def main():
    print(transform(lambda (x: Int) -> Int: x * 2, 4))
    print(transform(lambda (x: Int) {} -> Int: x + 1, 4))
    var factor = 3
    print(observe(lambda (x: Int) -> Int: x * factor, 5))
    factor = 2
    print(observe(lambda (x: Int) -> Int: x * factor, 5))
