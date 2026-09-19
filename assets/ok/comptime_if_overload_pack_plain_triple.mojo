# A mixed family beside a plain overload: three declarations of one name, two
# of them templates of different classes and one no template at all. The
# rebuild decides per declaration — the plain overload survives unchanged
# while both templates are replaced by their clones — and the plain overload
# still wins its own arity against the pack.
# requires: discovery


def kind[T: Copyable](a: T) -> Int:
    comptime if T == Int:
        return 1
    else:
        return 10


def kind[*Ts: Copyable](*xs: *Ts) -> Int:
    var n = 0
    comptime for i in range(Ts.length):
        n = n + 1
    return n


def kind(a: Int, b: Int) -> Int:
    return 55


def main():
    print(kind(3))
    print(kind(4, 5))
    print(kind(1, 2, 3))
