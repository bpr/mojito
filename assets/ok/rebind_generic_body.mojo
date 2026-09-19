# `rebind[Dest](value)` asserts that a parametric operand type resolves to
# `Dest` once instantiated, so a body holding one can only be judged per
# instantiation — with or without a `comptime if` to key it. The template is
# never checked with `T` symbolic; each clone asserts the equality, and a
# template no call instantiates is not judged at all.
@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var v: Self.T

    def get_int(self) -> Int:
        return rebind[Int](self.v)


def bump[T: Copyable](mut x: T):
    rebind[Int](x) += 1


def put[T: Copyable & Deinitable](mut x: T):
    rebind[String](x) = String("new")


def as_int[T: Copyable](x: T) -> Int:
    return rebind[Int](x)


# An abstract body reaches the template, and a `def` nested in it does too:
# both specialize per call to get there.
def show[T: Copyable](x: T):
    def twice[U: Copyable](y: U) -> Int:
        return as_int(y) * 2

    print(as_int(x), twice(x))


# Never instantiated: the pin does not judge such a template either.
def never_called[T: Copyable](x: T) -> Bool:
    return rebind[Bool](x)


def main():
    var v = 3
    bump(v)
    print(v)
    bump[Int](v)
    print(v)
    var s = String("old")
    put(s)
    print(s)
    show(7)
    print(Box[Int](9).get_int())
