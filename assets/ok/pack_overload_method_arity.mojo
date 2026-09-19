# Two type-pack overloads of one struct method, told apart by how many
# regular parameters precede the collector and by a keyword-only parameter.
# requires: discovery


@fieldwise_init
struct Box(Copyable, Movable):
    var v: Int

    def pick[*Ts: Writable](self, a: Int, *rest: *Ts) -> Int:
        return 1

    def pick[*Ts: Writable](self, a: Int, b: Int, *rest: *Ts) -> Int:
        return 2

    def tag[*Ts: Writable](self, *rest: *Ts) -> Int:
        return 1

    def tag[*Ts: Intable](self, *rest: *Ts, scale: Int) -> Int:
        return 2


def main():
    var b = Box(1)
    print(b.pick(1, "x"))
    print(b.pick(1, 2, "x"))
    print(b.tag("x"))
    print(b.tag(3, scale=2))
