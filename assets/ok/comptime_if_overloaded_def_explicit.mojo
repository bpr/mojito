# Explicit compile-time arguments to an overloaded compile-time-keyed `def`.
# `[Int]` names type arguments, not an overload, so the elaborator cannot
# choose syntactically: these calls are served from the checker's recorded
# instantiation exactly as inferred ones are, and both spellings of one
# instantiation reach the same clone.


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


def main():
    print(kind[Int](3))
    print(kind[Int](3, 4))
    print(kind[Bool](True))
    print(kind[Bool](True, False))
    # The explicit and inferred spellings of the same instantiation.
    print(kind(5))
    print(kind[Int](6))
