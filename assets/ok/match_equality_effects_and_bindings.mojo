# `case == expr` evaluates its expression and calls `__eq__` only when the
# case is reached, in case order; a bare `case name` binds an immutable
# reference to the subject (a place or a temporary, copyable or not), the
# subject stays usable, and a temporary subject is destroyed after the match.
struct Key(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __eq__(self, other: Self) -> Bool:
        print("eq", self.v, other.v)
        return self.v == other.v


struct Big(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def expected(v: Int) -> Key:
    print("expected", v)
    return Key(v)


def make() -> Big:
    return Big(3)


def main():
    var k = Key(2)
    match k:
        case == expected(1):
            print("one")
        case == expected(2):
            print("two")
        case _:
            print("other")
    match k:
        case x:
            print("bare", x.v)
    var s = String("hi")
    match s:
        case "no":
            print("no")
        case whole:
            print("whole", whole, s)
    print(s)
    var b = Big(1)
    match b:
        case x:
            print("bare", x.v)
    match make():
        case x:
            print("temp", x.v)
    print("end")
