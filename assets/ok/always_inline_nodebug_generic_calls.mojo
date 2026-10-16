# Calls to generic `@always_inline("nodebug")` functions are inlined before
# elaboration, their compile-time parameters bound to the call's: a literal
# argument, one over the caller's own parameters, an inferred or defaulted
# one, a method's own parameters, and a variadic pack's elements. Each
# prints what the call computes either way.

from std.bit import rotate_bits_left


@always_inline("nodebug")
def scaled[k: Int](x: Int) -> Int:
    return x * k


@always_inline("nodebug")
def ident[T: ImplicitlyCopyable & Movable](x: T) -> T:
    return x


@always_inline("nodebug")
def offset[k: Int = 5](x: Int) -> Int:
    return x + k


@always_inline("nodebug")
def pick[flag: Bool](x: Int) -> Int:
    comptime if flag:
        return x + 1
    return x - 1


@always_inline("nodebug")
def labelled[label: String](x: Int) -> String:
    return label + String(x)


struct Meter:
    var value: Int

    def __init__(out self, value: Int):
        self.value = value

    @always_inline("nodebug")
    def scaled[k: Int](self) -> Int:
        return self.value * k

    @always_inline("nodebug")
    def plus[T: Intable](self, x: T) -> Int:
        return self.value + Int(x)


@always_inline("nodebug")
def arity[*Ts: Copyable](t: Tuple[*Ts], x: Int) -> Int:
    return x + len(Ts)


@always_inline("nodebug")
def last[*values: Int]() -> Int:
    comptime n = len(values)
    return n * 10 + values[n - 1]


def relayed[*Us: Copyable](t: Tuple[*Us]) -> Int:
    return arity(t, 1)


def shifted[n: Int](x: Int) -> Int:
    return scaled[n + 1](x) + pick[n > 2](x)


def main():
    print(scaled[3](2), shifted[1](2), shifted[4](2))
    print(ident(7), ident[String]("s"), offset(1), offset[2](1))
    print(pick[True](1), pick[False](1), labelled["v="](2))
    var meter = Meter(4)
    print(meter.scaled[3](), meter.plus(2))
    var bits = UInt64(5)
    print(
        rotate_bits_left[0](bits),
        rotate_bits_left[4](bits),
        rotate_bits_left[8](bits),
    )
    var pair = (3, "a")
    print(arity(pair, 2), last[4, 5](), relayed(pair))
