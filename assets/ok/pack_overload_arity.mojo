# Two type-pack overloads of one name that differ in how many regular
# parameters precede the collector. The checker selects between them — a
# candidate whose collector takes at least one argument wins, then the one
# binding the most regular parameters — and the elaborator serves each call
# from that recorded selection rather than from the first declaration.
# requires: discovery


def pick[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 1


def pick[*Ts: Writable](a: Int, b: Int, *rest: *Ts) -> Int:
    return 2


def main():
    var x = 7
    print(pick(1, "x"))
    print(pick(1, 2))
    print(pick(1, 2, "x"))
    print(pick(x, x, x))
