# One name holding a compile-time-keyed declaration and three type-pack
# declarations. Each call is served by the declaration the checker selected.
# requires: discovery


def kind[T: Intable](a: T) -> Int:
    comptime if T == Int:
        return 1
    else:
        return 2


def kind[*Ts: Writable](first: String, *rest: *Ts) -> Int:
    return 10 + len(rest)


def kind[*Ts: Writable](first: Float64, second: Int, *rest: *Ts) -> Int:
    return 20 + len(rest)


def kind[*Ts: Writable](*rest: *Ts, sep: String) -> Int:
    return 30 + len(rest)


def main():
    print(kind(3))
    print(kind(String("x"), 1, 2))
    print(kind(1.5, 2, "z"))
    print(kind("p", "q", sep=","))
