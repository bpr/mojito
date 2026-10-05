# A `def` keyed on an `Int`, `Bool`, or `DType` value beside a type pack is
# served by its template: an explicit `scaled[3](7, "x")` binds `n` from the
# brackets and `Us` from the collected arguments, and an inferred call binds
# `dt` from its argument's lane slot.
def scaled[n: Int, *Us: Writable](*extra: *Us) -> Int:
    return n * len(extra)


def scaled_length[n: Int, *Us: Writable](*extra: *Us) -> Int:
    return n * Us.length + 1


def show[flag: Bool, *Ts: Writable](*args: *Ts):
    comptime if flag:
        print(len(args))
    else:
        print(0)


def lanes[dt: DType, *Ts: Writable](x: Scalar[dt], *args: *Ts) -> Int:
    return len(args)


def main():
    print(scaled[3](7, "x"))
    print(scaled[2]())
    print(scaled_length[4](1, 2.5, "z"))
    show[True](1, "a")
    show[False](1)
    print(lanes(Float32(1.5), 1, "x"))
    print(lanes(Int8(2)))
