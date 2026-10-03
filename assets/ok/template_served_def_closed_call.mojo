# Every closed call of a plain trait-bound `def` keeps its template, inferred
# (`show(n)`) or explicit (`show[Int](n)`): the elaborator instantiates the
# template's MIR, so no call mints a clone. A type argument no runtime
# parameter or result spells (`bytes[Int]()`) binds from the arguments the
# call records in MIR. Defaults, `raises`, a `var` parameter transferred out,
# a generic struct built over the parameter, and a call from another served
# `def` all run on the template. `rep` keeps its clone for its value
# parameter, and `total` for the `Tuple` it applies over `T`.
from std.sys import size_of
@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable, Movable, Deinitable):
    var v: Self.T

    def get(self) -> Self.T:
        return self.v.copy()


def show[T: Writable](x: T):
    print(x)


def make[T: Defaultable]() -> T:
    return T()


def bytes[T: AnyType]() -> Int:
    return size_of[T]()


def padded[T: Writable](x: T, width: Int = 4):
    print(width, x)


def checked[T: Writable](x: T, fail: Bool) raises:
    if fail:
        raise "checked failed"
    print(x)


def pass_through[T: Movable](var x: T) -> T:
    return x^


def twice[T: Copyable & Deinitable & Writable](x: T) -> Box[T]:
    var b = Box[T](x.copy())
    show(b.get())
    return b^


def rep[T: Writable, n: Int](x: T):
    for _ in range(n):
        print(x)


def total[T: Writable & Movable](pair: Tuple[Int, T]) -> Int:
    show(pair[1])
    return pair[0]


def main():
    var n = 3
    show(n)
    show[Int](n)
    show("hi")
    var i = make[Int]()
    print(i, bytes[Int](), bytes[Float64]())
    padded(7)
    padded(8, 2)
    try:
        checked(9, False)
        checked(10, True)
    except e:
        print("caught", e)
    var s = pass_through(String("moved"))
    print(s)
    var b = twice(5)
    print(b.get())
    rep[Int, 2](11)
    print(total((12, String("t"))))
