# A type-pack call inside a nested `def` and a whole-pack forward
# `take(*args)` each bind the callee's pack from the arguments at the call,
# as at top level.
def take[*Ts: Writable](*args: *Ts) -> Int:
    var n = 0
    comptime for i in range(args.__len__()):
        print(args[i])
        n += 1
    return n


def forward[*Ts: Writable](*args: *Ts) -> Int:
    return take(*args)


def outer[*Ts: Writable](*args: *Ts) -> Int:
    def inner() -> Int:
        var b = 2.5
        return take(b, 1)

    return inner() + take(*args)


def main():
    def local() -> Int:
        var b = String("x")
        return take(b, 1)

    print(local())
    var b = String("y")
    print(forward(b, 1, True))
    print(outer("a", 3))
