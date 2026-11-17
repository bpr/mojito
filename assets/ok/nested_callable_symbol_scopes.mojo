# A capture-free nested `def` or lambda at a callable slot is the symbol of
# its generator applied to the enclosing binders wherever it is declared:
# under a same-spelled sibling, inside a lambda, inside a capture-list
# closure, and inside an expression lifted to compile time.


def g[f: def() thin -> Int]():
    print(f())


def apply[f: def() thin -> Int]() -> Int:
    return f()


def spelled_twice[flag: Bool]():
    comptime if flag:

        def h() -> Int:
            return 1

        g[h]()
    else:

        def h(x: Int) -> Int:
            return x

        print(h(2))


def in_lambda():
    var l = lambda -> Int: (g[lambda -> Int: 11](), 0)[1]
    _ = l()


def in_closure[n: Int]():
    var x = 5

    def cl() {imm}:
        def inner() -> Int:
            return n

        g[inner]()
        print(x)

    cl()


def lifted[n: Int]():
    comptime v = apply[lambda -> Int: n * 10]()
    print(v + n)


def main():
    spelled_twice[True]()
    spelled_twice[False]()
    in_lambda()
    in_closure[3]()
    lifted[1]()
