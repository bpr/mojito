# Lambdas as higher-order-function arguments: a thin lambda binds to a plain
# `def(...) thin` contract. Passing a *capturing* lambda as a runtime
# argument is Mojito-only — upstream takes a capturing callable only as a
# compile-time parameter, and refuses a lambda in that position — so that
# half is the `capturing-lambda-argument` conformance case.
def transform(f: def(x: Int) thin -> Int, value: Int) -> Int:
    return f(value)

def observe[origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int](value: Int) -> Int:
    return f(value)

def main():
    print(transform(lambda (x: Int) -> Int: x * 2, 4))
    print(transform(lambda (x: Int) {} -> Int: x + 1, 4))
    var factor = 3

    @parameter
    def scale(x: Int) -> Int:
        return x * factor

    print(observe[scale](5))
    factor = 2
    print(observe[scale](5))
