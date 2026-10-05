# A generic `def` holding a `comptime for` over a list, set, or dictionary
# display of literals is served by its template, as a `range` loop is: the
# checker types the body once with the loop variable a symbolic binder of the
# element's type, MIR carries the loop as a `comptime_for.elements` header,
# and `native::mono` unrolls it under each instance's bindings. A list of
# integers, strings, and booleans, a set's distinct elements, a dictionary's
# keys, a `comptime if` over the variable, a compile-time `break` and
# `continue`, a list loop around a `range` loop over a parameter, and a pack
# element read at a list element all run on one template. Output matches
# the pin.


def scaled[n: Int]():
    comptime for x in [1, 2, 3]:
        print("scaled", x * n)


def labelled[T: Writable](value: T):
    comptime for label in ["a", "b"]:
        print(label, value)


def flags[n: Int]():
    comptime for flag in [True, False]:
        comptime if flag:
            print("on", n)
        else:
            print("off", n)


def distinct[n: Int]():
    comptime for k in {3, 1, 3, 2}:
        print("set", k + n)


def keys[n: Int]():
    comptime for key in {"x": 1, "y": 2, "x": 3}:
        print("key", key, n)


def until[n: Int]():
    comptime for x in [1, 2, 3, 4]:
        comptime if x == n:
            break
        comptime if x == 1:
            continue
        print("until", x)


def grid[n: Int]():
    comptime for row in [10, 20]:
        comptime for col in range(n):
            print("grid", row + col)


def picked[*Ts: Writable](*args: *Ts):
    comptime for i in [1, 0]:
        print("picked", args[i])


def main():
    scaled[2]()
    scaled[5]()
    labelled(7)
    labelled("z")
    flags[4]()
    distinct[10]()
    keys[0]()
    until[4]()
    grid[2]()
    picked(1, "x")
