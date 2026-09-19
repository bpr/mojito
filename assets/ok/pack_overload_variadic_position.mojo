# Two type-pack overloads that agree on every regular parameter's name and
# type and differ only in where the collector sits: a parameter after it is
# keyword-only. They are distinct declarations, and a request tells them apart
# by the collector's position.
# requires: discovery


def pos[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def pos[*Ts: Writable](*rest: *Ts, a: Int) -> Int:
    return 2


def main():
    print(pos(1, "x"))
    print(pos("x", a=1))
