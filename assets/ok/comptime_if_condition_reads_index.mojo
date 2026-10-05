# A `comptime if` condition that applies a function inside a template-served
# `comptime for` reads the loop's index: the thunk the condition lowers to
# declares every binder in scope at the condition, the enclosing loops'
# indices included, and reads an index as a parameter reference, so each
# unrolled copy demands the thunk at its own index. Nested loops, two loops
# of one name, an inner index shadowing an outer one or a struct parameter,
# a display loop, and a loop inside a `try` region all decide per copy.
def is_even(k: Int) -> Bool:
    return k % 2 == 0


def may(k: Int) raises -> Int:
    if k > 100:
        raise Error("big")
    return k


def ranged[n: Int]():
    comptime for i in range(n):
        comptime if is_even(i):
            print("even", i)
        else:
            print("odd", i)


def nested[n: Int]():
    comptime for i in range(n):
        comptime for j in range(2):
            comptime if is_even(i + j + n):
                print("pair", i, j)


def twice[n: Int]():
    comptime for i in range(n):
        comptime if is_even(i):
            print("first", i)
    comptime for i in range(n):
        comptime if is_even(i + 1):
            print("second", i)


def shadowed[n: Int]():
    comptime for i in range(n):
        comptime for i in range(2):
            comptime if is_even(i):
                print("inner", i)


def listed[n: Int]():
    comptime for x in [1, 2, 4]:
        comptime if is_even(x + n):
            print("element", x)


def guarded[n: Int]():
    try:
        comptime for i in range(n):
            comptime if is_even(i):
                print("try", may(i))
    except e:
        print(e)


struct Grid[i: Int]:
    def __init__(out self):
        pass

    def go(self):
        comptime for i in range(2):
            comptime if is_even(i + Self.i):
                print("grid", i)


def main():
    ranged[4]()
    nested[3]()
    twice[3]()
    shadowed[2]()
    listed[0]()
    guarded[3]()
    Grid[1]().go()
