# A surviving trait-bound `def` taking a `var` parameter derives its instances
# from the checked template (`docs/notes/instantiation-from-template.md`,
# class FunctionBody): the parameter is bound owned from its declared
# convention alone, and the body may consume it through a method taking its
# receiver (`token^.close()`, `allocation^.unsafe_leak()` in the bundled
# `dealloc`), transfer it into a local, or hand it on by value. Whether the
# callee is a named destructor is the receiver struct's declaration, which no
# instance changes.
from std.memory import Layout, alloc, dealloc


@explicit_destroy("close the token")
struct Token[T: AnyType](Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self) -> Int:
        return self.id


def finish[T: AnyType](var token: Token[T]) -> Int:
    return token^.close()


def count_one[T: Copyable & Deinitable](var item: T) -> Int:
    return 1


def weigh[T: Copyable & Deinitable](var item: T, extra: Int) -> Int:
    var held = item^
    return count_one(held^) + extra


def main():
    print(finish(Token[Int](1)), finish(Token[String](2)))
    print(weigh(3, 4), weigh(String("x"), 5))
    var ints = alloc(Layout[Int](count=2))
    var strings = alloc(Layout[String](count=1))
    print(ints.layout().count(), strings.layout().count())
    dealloc(ints^)
    dealloc(strings^)
