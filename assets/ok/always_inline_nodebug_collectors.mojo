# Each call that collects or spreads arguments into an `@always_inline("nodebug")`
# callee is inlined: a homogeneous collector, a heterogeneous one, an owned
# one, a spread of the caller's pack, a forwarded `**kwargs^`, and the
# consuming `Tuple` members, whose bodies apply a compile-time function to
# the owner's pack, on a copied receiver.
@always_inline("nodebug")
def total(*xs: Int) -> Int:
    var s = 0
    for x in xs:
        s += x
    return s


@always_inline("nodebug")
def names(*xs: String) -> Int:
    var n = 0
    for x in xs:
        n += x.byte_length()
    return n


@always_inline("nodebug")
def owned(var *xs: String) -> Int:
    return len(xs)


@always_inline("nodebug")
def emit[*Ts: Writable](mut s: String, *args: *Ts):
    comptime for i in range(args.__len__()):
        args[i].write_to(s)


@always_inline("nodebug")
def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        print(args[i])


def outer[*Ts: Writable](*args: *Ts):
    show(*args)


@always_inline("nodebug")
def inner(var **kwargs: Int) -> Int:
    return len(kwargs)


@always_inline("nodebug")
def relay(var **kwargs: Int) -> Int:
    return inner(**kwargs^)


struct Sink:
    var total: Int

    def __init__(out self):
        self.total = 0

    @always_inline("nodebug")
    def take(mut self, *xs: Int):
        for x in xs:
            self.total += x


def main():
    print(total(1, 2, 3), total())
    var a = String("ab")
    print(names(a, String("cde")), owned(a, String("f")))
    var s = String()
    emit(s, 1, "x", 2.5)
    print(s)
    show(1, "a")
    outer(2, "b")
    print(relay(a=1, b=2))
    var sink = Sink()
    sink.take(1, 2)
    sink.take()
    print(sink.total)
    var pair = Tuple(1, True)
    var suffix = Tuple(2.5)
    var joined = pair.concat(suffix)
    print(joined[0], joined[1], joined[2])
    var reversed = pair.reverse()
    print(reversed[0], reversed[1])
    print(len(Tuple().reverse()))
