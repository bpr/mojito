# A compile-time-keyed generic `def` overloaded with a type-pack one. The two
# declarations are templates of different specialization classes under one
# name: the class is a property of a declaration, and the request's selected
# declaration is what tells a call which class serves it.
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


def through(a: Int) -> Int:
    # Both members reached from another body, declared after the family, so
    # each one's survival is observable rather than incidental.
    return kind(a) + kind(a, a, a)


def main():
    print(kind(3))
    print(kind(True))
    print(kind(1, 2))
    print(kind(1, True, 3))
    print(through(7))
