# A generic `def` holding a `comptime for` over a `range` is served by its
# template: the checker types the body once with the index symbolic, MIR
# carries the loop as the `comptime_for` header, and `native::mono` unrolls it
# under each instance's bindings. Arithmetic bounds, a loop nested in another
# over its index, a value parameter applied to the index (`tag[j]()`), a
# `comptime if` over the index, a loop inside a `try` region, and a
# compile-time `break` and `continue` all run on one template. Output matches
# the pin, destructor order included.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __deinit__(deinit self):
        print("del", self.s)


def tag[k: Int]() -> Int:
    return k * 10


def odds[n: Int]() -> Int:
    var total = 0
    comptime for i in range(1, n + 1, 2):
        total += i
    return total


def triangle[n: Int]() -> Int:
    var total = 0
    comptime for i in range(n):
        comptime for j in range(i):
            total += tag[j]()
        comptime if i % 2 == 0:
            total += 1
    return total


def guarded[n: Int]() raises -> Int:
    var acc = 0
    try:
        comptime for i in range(n):
            if i == 2:
                raise Error("two")
            acc += i
    except e:
        print("caught", e)
        return -acc
    return acc


def nested_break[n: Int]():
    comptime for i in range(n):
        var t = Thing("t" + String(i))
        comptime for j in range(3):
            comptime if j == 1:
                break
            print("i", i, "j", j)
        comptime if i == 1:
            continue
        print("end", i)


def main() raises:
    print(odds[0](), odds[5](), odds[6]())
    print(triangle[0](), triangle[1](), triangle[4]())
    print(guarded[2](), guarded[4]())
    nested_break[3]()
