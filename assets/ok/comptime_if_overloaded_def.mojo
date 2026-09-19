# Inferred calls to an overloaded `def` whose every body is keyed by a
# `comptime if` over the declaration's own type parameter. Overload selection
# stays the checker's: the elaborator serves each call from the checker's
# recorded instantiation, which names the selected overload by its runtime
# parameter names.


def kind[T: Copyable](a: T) -> Int:
    comptime if T == Int:
        return 1
    else:
        return 10


def kind[T: Copyable](a: T, b: T) -> Int:
    comptime if T == Int:
        return 2
    else:
        return 20


# The same family declared with the wider overload first: which declaration a
# call selects must not depend on declaration order.
def rank[T: Copyable](a: T, b: T) -> Int:
    comptime if T == Int:
        return 4
    else:
        return 40


def rank[T: Copyable](a: T) -> Int:
    comptime if T == Int:
        return 3
    else:
        return 30


def main():
    print(kind(3))
    print(kind(3, 4))
    print(kind(True))
    print(kind(True, False))
    print(rank(3))
    print(rank(3, 4))
    print(rank(True))
