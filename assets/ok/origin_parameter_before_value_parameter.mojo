# An origin parameter ahead of a value parameter: positional arguments bind
# in order, so `W[ImmStaticOrigin]` leaves the defaulted `forward` to its
# default, and `f[ImmStaticOrigin, 3]()` binds `n` to 3.
struct W[origin: ImmOrigin, forward: Bool = True](ImplicitlyCopyable):
    var s: StringSpan[Self.origin]

    def __init__(out self, s: StringSpan[Self.origin]):
        self.s = s

    def direction(self) -> String:
        comptime if Self.forward:
            return "forward"
        else:
            return "backward"


def f[o: ImmOrigin, n: Int]() -> Int:
    return n


def make() -> W[ImmStaticOrigin]:
    return W[ImmStaticOrigin](StaticString("ab"))


def make_backward() -> W[ImmStaticOrigin, False]:
    return W[ImmStaticOrigin, False](StaticString("cd"))


def main():
    print(make().s, make().direction())
    print(make_backward().s, make_backward().direction())
    var t = String("xy")
    var w = W[ImmOrigin(origin_of(t))](StringSpan(t))
    print(w.s, w.direction())
    print(f[ImmStaticOrigin, 3]())
