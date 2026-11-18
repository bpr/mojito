# expect: 'h' is already declared in this scope
# A nested `def`'s identity is scoped to its enclosing function, though its
# name is looked up in its block, so a second `def h() -> Int` in a sibling
# block of the same body redefines it, as at the pin ("redefinition of
# function 'h' with identical signature").


def g[f: def() thin -> Int]():
    print(f())


def run[flag: Bool]():
    comptime if flag:

        def h() -> Int:
            return 1

        g[h]()
    else:

        def h() -> Int:
            return 2

        g[h]()


def main():
    run[True]()
    run[False]()
