# Two same-arity generic overloads of one method, `pick(a: T, b: T)` and
# `pick(a: T, b: Int)`, that a call's type arguments can specialize alike.
# Each call mints only the overload it selects, so `pick(2, 3)` ranks the
# `b: Int` overload first and prints `2`, as the pinned Mojo does, on a plain
# struct, through a trait bound, and on a generic struct's instance.
# requires: discovery


trait Picker:
    def pick[T: Copyable](self, a: T, b: T) -> T:
        ...

    def pick[T: Copyable](self, a: T, b: Int) -> T:
        ...


@fieldwise_init
struct First(Picker):
    var tag: Int

    def pick[T: Copyable](self, a: T, b: T) -> T:
        return b.copy()

    def pick[T: Copyable](self, a: T, b: Int) -> T:
        return a.copy()


@fieldwise_init
struct Holder[U: Copyable & Deinitable]:
    var item: Self.U

    def pick[T: Copyable](self, a: T, b: T) -> T:
        return b.copy()

    def pick[T: Copyable](self, a: T, b: Int) -> T:
        return a.copy()


def use[P: Picker](p: P):
    print(p.pick(4, 5))


def main():
    print(First(0).pick(2, 3))
    print(First(0).pick(String("a"), String("b")))
    print(First(0).pick(String("a"), 3))
    use(First(0))
    print(Holder[Int](1).pick(6, 7))
    print(Holder[Int](1).pick(String("c"), String("d")))
    print(Holder[Int](1).pick(String("c"), 8))
