# An error raised by a case's `__eq__` or by its body propagates through the
# enclosing `try`, and a temporary subject is destroyed on the way out.
struct Key(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __eq__(self, other: Self) raises -> Bool:
        if other.v == 0:
            raise Error("zero")
        return self.v == other.v


struct Big(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def make() -> Big:
    return Big(3)


def main():
    var k = Key(2)
    try:
        match k:
            case == Key(0):
                print("zero")
            case _:
                print("other")
    except e:
        print("raised", e)
    try:
        match make():
            case x if x.v == 3:
                print("in", x.v)
                raise Error("out")
            case _:
                print("no")
    except e:
        print("caught", e)
    finally:
        print("finally")
    print("end")
