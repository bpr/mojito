# A trait listed twice must carry the same constraint both times; an entry
# without `where` is the constraint `True`.
# expect: trait 'Movable' appears multiple times in the conformance list with different constraints


@fieldwise_init
struct Box[T: AnyType](Movable where conforms_to(T, Copyable), Movable):
    var x: Int


def main():
    var b = Box[Int](1)
    print(b.x)
