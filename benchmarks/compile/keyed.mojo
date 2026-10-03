# Compile-time-keyed and variadic `def`s at several instantiations: the bodies
# the parametric-MIR plan's stages P3a and P3b move. User `def`s fold a
# `comptime if` on a value or a type parameter, unroll a `comptime for`, and
# expand a type pack; bundled `rotate_bits_left` folds its own `comptime if`;
# a generic struct's method folds a `comptime if` on `Self.T`.
from std.bit import rotate_bits_left


def scale[n: Int](x: Int) -> Int:
    comptime if n == 0:
        return 0
    elif n == 1:
        return x
    else:
        return x * n


def describe[T: Writable](x: T) -> String:
    comptime if T == Int:
        return "int " + String(x)
    elif T == String:
        return "str " + String(x)
    else:
        return "other " + String(x)


def unrolled_sum[n: Int](x: Int) -> Int:
    var total = 0
    comptime for i in range(n):
        total += x * i
    return total


def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(Ts.length):
        print(args[i], end=" ")
    print()


def count[*Ts: AnyType](*args: *Ts) -> Int:
    return len(args)


struct Cell[T: Copyable & Deinitable & Writable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def label(self) -> String:
        comptime if Self.T == Int:
            return "int cell " + String(self.value)
        else:
            return "cell " + String(self.value)


def main():
    print(scale[0](5), scale[1](5), scale[3](5), scale[7](5))
    print(describe(1), describe(String("a")), describe(2.5), describe(True))
    print(unrolled_sum[2](3), unrolled_sum[4](3), unrolled_sum[8](3))
    show(1, "two", 3.0)
    show(String("x"), True)
    show(4, 5, 6, 7)
    print(count(1, 2), count("a", 2.0, True))
    var one: UInt64 = 1
    print(rotate_bits_left[0](one), rotate_bits_left[4](one), rotate_bits_left[8](one))
    print(Cell(9).label(), Cell(String("b")).label(), Cell(1.5).label())
