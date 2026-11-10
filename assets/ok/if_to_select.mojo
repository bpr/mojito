# `canonicalize` turns an elif-free `if` over register-passable trivial
# results into `pop.select`, as the pin's `EmptyIfToSelect` and
# `IfYieldSelect` do; an `if` with elifs, a call in an arm, and a
# non-speculatable arm keep their branch.


@no_inline
def f(c: Bool) -> Int:
    var x = 1 if c else 2
    return x


@no_inline
def g(c: Bool, a: Int, b: Int) -> Int:
    var y: Int
    if c:
        y = a
    else:
        y = b
    return y


@no_inline
def h(c: Bool, a: Int) -> Int:
    var y = a
    if c:
        y = 7
    return y


@no_inline
def e(c: Bool, a: Int, b: Int) -> Int:
    var y = a
    var z = b
    if c:
        y = b
        z = a
    return y - z


@no_inline
def fl(c: Bool, a: Float64) -> Float64:
    return a if c else 0.5


@no_inline
def bb(c: Bool, d: Bool) -> Bool:
    return d if c else False


@fieldwise_init
struct P(TrivialRegisterPassable, Writable):
    var x: Int
    var y: Int


@no_inline
def structsel(c: Bool, a: P, b: P) -> P:
    return a if c else b


@no_inline
def deep(c: Bool, d: Bool, a: Int, b: Int) -> Int:
    var y = 0
    if c:
        if d:
            y = a
        else:
            y = b
    return y


@no_inline
def one_arm_ret(c: Bool, a: Int) -> Int:
    if c:
        return a
    return 3


@no_inline
def k(c: Bool, d: Bool, a: Int, b: Int) -> Int:
    var y: Int
    if c:
        y = a
    elif d:
        y = b
    else:
        y = 9
    return y


@no_inline
def foo(a: Int) -> Int:
    return a + 3


@no_inline
def callarm(c: Bool, a: Int, b: Int) -> Int:
    return foo(a) if c else b


@no_inline
def divarm(c: Bool, a: Int, b: Int) -> Int:
    return a // b if c else b


@no_inline
def printarm(c: Bool, a: Int, b: Int) -> Int:
    var y = b
    if c:
        print("then")
        y = a
    return y


@always_inline
def pick(c: Bool, a: Int, b: Int) -> Int:
    return a if c else b


@no_inline
def use_true(a: Int, b: Int) -> Int:
    return pick(True, a, b)


@no_inline
def use_same(c: Bool, a: Int) -> Int:
    return pick(c, a, a)


def main():
    var n = String("abc").byte_length()
    var c = n == 3
    var d = not c
    print(f(c), f(d), g(c, n, 4), g(d, n, 4), h(c, n), h(d, n))
    print(e(c, n, 4), e(d, n, 4), fl(c, 1.5), fl(d, 1.5), bb(c, c), bb(d, c))
    print(structsel(c, P(1, 2), P(3, 4)), structsel(d, P(1, 2), P(3, 4)))
    print(deep(c, d, n, 2), deep(c, c, n, 2), deep(d, c, n, 2))
    print(one_arm_ret(c, n), one_arm_ret(d, n), k(c, d, n, 2), k(d, c, n, 2))
    print(k(d, d, n, 2), callarm(c, n, 2), callarm(d, n, 2))
    print(divarm(c, n, 4), divarm(d, n, 4), printarm(c, n, 4), printarm(d, n, 4))
    print(use_true(n, 2), use_same(c, n))
