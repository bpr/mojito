# Bool-bodied generic comptime aliases (predicate aliases): the body is a
# compile-time proposition, applicable exactly where conforms_to and the
# IsTrivially* predicates are — where clauses, conditional-conformance
# conditions, and comptime if — and a predicate alias may expand an earlier
# one.

comptime IsSmallCopy[T: AnyType] = conforms_to(T, Copyable) and IsTriviallyCopyable[T]
comptime IsBigOrLinear[T: AnyType] = not IsSmallCopy[T]


struct Plain(Copyable, Deinitable, Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


struct Wrap[T: Copyable & Deinitable & Movable](
    Copyable where IsSmallCopy[T], Deinitable, Movable
):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^


def dup[T: Copyable & Deinitable](value: T) -> T where IsSmallCopy[T]:
    return value.copy()


def main():
    comptime if IsSmallCopy[Int]:
        print("Int is a small copy")
    comptime if IsBigOrLinear[String]:
        print("String is not")
    print(dup(7))
    print(dup(Plain(3)).x)
    var wrapped = Wrap(Plain(4))
    var copied = wrapped.copy()
    print(copied.item.x)
