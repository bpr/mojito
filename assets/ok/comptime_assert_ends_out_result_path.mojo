# A `comptime assert` whose condition folds to `False` ends its path, as the
# pin's constraint-failure terminator does: an `out result` function whose
# untaken arm ends in one never reaches that arm's end, so `result` need not
# be set there, and a value owned at the assertion is never destroyed past it.

struct Noisy(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)


def first_two[w: SIMDLength](b: SIMD[_, w], out result: type_of(b)):
    comptime if w == 2:
        return b
    else:
        comptime assert False, "x"


def pick[n: Int](out result: Noisy):
    comptime if n == 1:
        result = Noisy(1)
    elif n == 2:
        var t = Noisy(9)
        comptime assert False, "never two"
    else:
        result = Noisy(n)
        return


def signed[n: Int](x: Int, out result: Int):
    comptime if n == 1:
        result = x
    else:
        if x > 0:
            comptime assert False, "never"
        else:
            result = -x


def main():
    print(first_two(SIMD[DType.int32, 2](1, 2)))
    print(pick[1]().n)
    print(pick[3]().n)
    print(signed[1](5))
