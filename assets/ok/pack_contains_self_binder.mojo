struct Holder[T: Copyable & Writable]:
    var x: Int

    def __init__(out self):
        self.x = 0

    def has_self[*Us: Writable](self, *extra: *Us) -> Bool:
        return Us.contains[Self.T]()

    def cond_self[*Us: Writable](self, *extra: *Us) -> Bool:
        comptime if Us.contains[Self.T]():
            return True
        return False


def nested[T: Copyable, *Us: Writable](x: T, *extra: *Us) -> Bool:
    comptime if Us.contains[T]():
        return True
    return False


def main():
    print(Holder[Int]().cond_self(1, "x"))
    print(Holder[Float64]().cond_self(1, "x"))
    print(Holder[Int]().has_self(1, "x"))
    print(Holder[Float64]().has_self(1, "x"))
    print(nested(1, 2))
    print(nested(1.5, 2))
