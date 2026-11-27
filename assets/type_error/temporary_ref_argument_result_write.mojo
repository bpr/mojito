# A temporary bound to a `ref[o]` parameter is lent immutably, so `o` binds
# an immutable origin and the call's `ref[o]` result is no assignment
# target.
# expect: expression must be mutable in assignment
struct S:
    var x: Int

    def __init__(out self):
        self.x = 1


def by[o: Origin](ref[o] s: S) -> ref[o] S:
    return s


def main():
    by(S()).x = 4
    print("unreachable")
