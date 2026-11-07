# A `comptime assert` whose condition the pin's parameter canonicalizer
# folds to `False` ends its path even when the condition reads a parameter:
# `n != n` and `n - n == 1` fold, and `False and n == 2` folds because `and`
# selects its left operand when that operand is `False`.


def ne[n: Int]() -> Int:
    comptime if n == 1:
        return 1
    else:
        comptime assert n != n, "x"


def difference[n: Int]() -> Int:
    comptime if n == 1:
        return 2
    else:
        comptime assert n - n == 1, "x"


def conjunction[n: Int]() -> Int:
    comptime if n == 1:
        return 3
    else:
        comptime assert False and n == 2, "x"


def main():
    print(ne[1]())
    print(difference[1]())
    print(conjunction[1]())
