# A mixed family whose pack member cannot be named by the request's parameter
# names or types. A variadic parameter is caller-visible but is spelled by
# neither key, so the pack declaration and the nullary one both claim the
# empty list; only whether the request's own arguments bind a declaration's
# parameters tells them apart.
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


def kind() -> Int:
    return 77


def main():
    print(kind(3))
    print(kind(1, 2))
    print(kind())
