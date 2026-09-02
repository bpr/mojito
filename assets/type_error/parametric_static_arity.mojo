# A parametric static's explicit receiver arguments bind against the struct's
# parameter list; surplus arguments are rejected with the receiver solver's
# own diagnostic rather than a generic overload failure.
# expect: type 'Box' expects 1 type argument(s), got 2


struct Box[T: Copyable & Movable]:
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    @staticmethod
    def filled(var item: Self.T) -> Self:
        return Box(item^)


def main():
    var a = Box[Int, Int].filled(7)
    print(a.item)
