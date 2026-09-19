# Two type-pack overloads whose signatures differ only in the pack's bounds.
# Neither the parameter names nor the parameter types of a specialization
# request spell a collector, so the request carries the collector's own key.
# requires: discovery


@fieldwise_init
struct OnlyInt(Copyable, Intable, Movable):
    var v: Int

    def __int__(self) -> Int:
        return self.v


def show[*Ts: Writable](*args: *Ts) -> Int:
    return 1


def show[*Ts: Intable](*args: *Ts) -> Int:
    return 2


def main():
    print(show("a", "b"))
    print(show(OnlyInt(1), OnlyInt(2)))
