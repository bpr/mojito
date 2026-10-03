# A `def` holding a `comptime if` is served by its template: the body is
# checked once with its binders symbolic, MIR carries the region as a branch
# on a parameter expression, the ownership analysis decides it as the `if`
# of the same shape, and the elaborator keeps the taken arm with the destroys
# placed in it. One template serves a value-keyed `scale[n]` with an `elif`
# chain and a nested runtime `if`, a type-keyed `label[T]`, a region inside a
# `try`, and a value one arm alone consumes, destroyed at the other arm's
# entry — the pin's order.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __del__(deinit self):
        print("del", self.s)


def consume(var t: Thing):
    print("consume", t.s)


def scale[n: Int](x: Int) -> Int:
    comptime if n == 0:
        return 0
    elif n == 1:
        if x > 3:
            return x + 100
        return x
    else:
        return x * n


def label[T: Writable](x: T) -> String:
    comptime if T == Int:
        return "int " + String(x)
    elif T == String:
        return "str " + String(x)
    else:
        return "other " + String(x)


def guarded[n: Int]() raises -> Int:
    try:
        comptime if n > 2:
            raise Error("big")
        return n
    except e:
        print("caught", e)
        return -1


def one_arm[n: Int]():
    var a = Thing(String("a"))
    var b = Thing(String("b"))
    print("before")
    comptime if n > 0:
        consume(a^)
        print("then", b.s)
    else:
        print("else", b.s)
    print("after")


def main() raises:
    print(scale[0](5), scale[1](2), scale[1](5), scale[3](5))
    print(label(1), label(String("a")), label(2.5))
    print(guarded[1](), guarded[5]())
    one_arm[1]()
    print("--")
    one_arm[0]()
