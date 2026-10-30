# A signature applies a function whose parameter defaults to a `comptime`
# member of a struct declared in the same module (`t: Tgt = Tgt.current()`):
# the function classifies once the struct's members have registered, so
# `SIMD[DType.float32, tw()]` is its application, as at the pin.
struct Tgt[_v: Int = 3, //](TrivialRegisterPassable):
    var _dummy: Int

    @always_inline("builtin")
    def __init__(out self):
        self._dummy = 0

    comptime current = Tgt[_v=2]


def tw[t: Tgt[_v=2] = Tgt.current()]() -> Int:
    return t._v * 2


def g(x: SIMD[DType.float32, tw()]) -> Int:
    return Int(x[1])


def main():
    print(tw())
    print(g(SIMD[DType.float32, tw()](2.0)))
# stdout: 4
# stdout: 2
