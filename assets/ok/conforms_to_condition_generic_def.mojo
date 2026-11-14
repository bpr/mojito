# `comptime if conforms_to(T, X):` over a plain `def`'s parameter decides per
# instance, with `T` inferred from the value argument as at the pin: a
# `Movable` bound refined to `Copyable`, a conjunction with an `elif`, and a
# negated query falling through to a trailing `return`.


struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def tag[T: Movable](x: T) -> String:
    comptime if conforms_to(T, Copyable):
        return "copyable"
    else:
        return "move-only"


def tag2[T: AnyType](x: T) -> String:
    comptime if conforms_to(T, Copyable) and conforms_to(T, Writable):
        return "cw"
    elif conforms_to(T, Movable):
        return "mov"
    else:
        return "other"


def tag3[T: Movable]() -> Int:
    comptime if not conforms_to(T, Copyable):
        return 0
    return 1


def main():
    print(tag(1))
    print(tag(M(1)))
    print(tag("s"))
    print(tag2(1))
    print(tag2(M(2)))
    print(tag3[Int](), tag3[M]())
