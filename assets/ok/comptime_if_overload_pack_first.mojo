# The same mixed family with the type-pack member declared first. Which
# declaration a call selects is the checker's to decide and must not depend on
# declaration order: the name-keyed template registry keeps only one
# declaration, so resolving a call against it used to answer 99 here.
# requires: discovery


def kind[*Ts: Copyable](*xs: *Ts) -> Int:
    var n = 0
    comptime for i in range(Ts.length):
        n = n + 1
    return n


def kind[T: Copyable](a: T) -> Int:
    comptime if T == Int:
        return 1
    else:
        return 10


def main():
    print(kind(3))
    print(kind(True))
    print(kind(1, 2))
    print(kind(1, True, 3))
