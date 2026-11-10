# A `def`'s callable-value parameter takes a function symbol, part of the
# instance's identity as a struct's is: `g[seven]` and `g[eight]` are
# distinct instances, a forwarded binder (`g[f]` inside `fwd`) names the
# enclosing instance's function, a struct applied to the binder (`S[f]`,
# in a body and in a signature) calls it through `Self.f`, and a closure
# naming the binder is its storage's method, a first-class value.


def seven() -> Int:
    return 7


def eight() -> Int:
    return 8


def g[f: def() thin -> Int]() -> Int:
    return f()


def fwd[f: def() thin -> Int]() -> Int:
    return g[f]() + 1


struct S[f: def() thin -> Int]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return self.x + Self.f()


def through_struct[f: def() thin -> Int]() -> Int:
    return S[f](2).get()


def make[f: def() thin -> Int](x: Int) -> S[f]:
    return S[f](x)


def twice[F: def() -> Int](f: F) -> Int:
    return f() * 10 + f()


def callable_frame[f: def() thin -> Int](v: Int) -> Int:
    var x = v

    def body() {var x} -> Int:
        return x + f()

    return body() + twice(body)


def main():
    print(g[seven](), g[eight]())
    print(fwd[seven]())
    print(through_struct[seven]())
    print(make[eight](3).get())
    print(callable_frame[seven](1), callable_frame[eight](1))
