# A callable binder forwarded to another call (`g[f]()` inside `fwd[f:
# def() thin -> Int]`) is the call's parameter attribute alone, upstream's
# `#kgen.param.decl.ref` to the enclosing binder, as is a `#kgen.bind_params`
# over a generic one (`g[h[2]]()`): positionally, by keyword, at a method, a
# static method, a struct's own binder (`Self.f`), and through a generic
# callable binder (`F[f]()`).


struct T:
    @staticmethod
    def seven() -> Int:
        return 7


struct H:
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def m[f: def() thin -> Int](self) -> Int:
        return self.k + f()

    @staticmethod
    def s[f: def() thin -> Int]() -> Int:
        return f() * 2


struct S[f: def() thin -> Int]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return self.x + call0[Self.f]()


def call0[f: def() thin -> Int]() -> Int:
    return f()


def g[f: def() thin -> Int, n: Int]() -> Int:
    return f() + n


def fwd[f: def() thin -> Int]() -> Int:
    return call0[f]()


def fwd_all[f: def() thin -> Int]() -> Int:
    var h = H(1)
    return g[f, 3]() + h.m[f]() + H.s[f]() + g[n=1, f=f]()


def ap[
    F: def[f: def() thin -> Int]() thin -> Int, f: def() thin -> Int
]() -> Int:
    return F[f]()


def twice[n: Int]() -> Int:
    return n * 2


def run[h: def[n: Int]() thin -> Int]() -> Int:
    return call0[h[2]]()


def main():
    print(fwd[T.seven]())
    print(fwd_all[T.seven]())
    print(S[T.seven](1).get())
    print(ap[call0, T.seven]())
    print(run[twice]())
