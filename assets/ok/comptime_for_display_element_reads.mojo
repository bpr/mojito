# A `comptime for` display whose element calls a `def` returning a scalar,
# subscripts a compile-time list, or computes a float is served by the
# template like any other display over the binders: the check types each
# element, MIR lifts the display as a function, and the elaborator runs it
# per instance. A string literal beside a `String` joins it.
comptime SQUARES = [0, 1, 4, 9, 16]


def twice(n: Int) -> Int:
    return n * 2


def label(n: Int) -> String:
    return "n" + String(n)


def even(n: Int) -> Bool:
    return n % 2 == 0


def calls[n: Int]():
    comptime for x in [twice(n), n]:
        print(x)
    comptime for s in [label(n), "z"]:
        print(s)
    comptime for b in [even(n), not even(n)]:
        print(b)


def floats[n: Int]():
    comptime for x in [Float64(n) * 0.5, 1.5]:
        print(x)


def subscripts[n: Int]():
    comptime CUBES = [0, 1, 8, 27]
    comptime for x in [SQUARES[n], CUBES[n], SQUARES[twice(n)]]:
        print(x)


def main():
    calls[3]()
    calls[4]()
    floats[3]()
    subscripts[1]()
    subscripts[2]()
