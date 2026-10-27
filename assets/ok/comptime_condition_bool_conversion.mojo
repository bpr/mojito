# A compile-time condition that is not a `Bool` converts through `Bool(x)`,
# as at the pin: an integer leaf is `x != 0`, and any other leaf (a float
# expression) is the function the check lifts for its conversion. A layout
# query under a comparison folds under the target in a condition and in a
# bracket argument alike.
from std.sys import size_of


def show[b: Bool]():
    print(b)


def ints[n: Int]():
    comptime if n:
        print("n is nonzero")
    else:
        print("n is zero")
    comptime if n - 2:
        print("n is not 2")
    comptime assert n, "n must be nonzero"


def floats[y: Float64]():
    comptime if y * 2.0:
        print("y is nonzero")
    else:
        print("y is zero")


def main():
    ints[2]()
    ints[3]()
    floats[1.5]()
    floats[0.0]()
    comptime if 3:
        print("three")
    comptime assert 3, "int"
    show[size_of[Int]() == 8]()
    comptime if size_of[Int]() == 8:
        print("64-bit")
