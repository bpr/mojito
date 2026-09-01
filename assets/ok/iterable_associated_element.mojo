trait Iterable:
    comptime Element: Copyable & Movable

@fieldwise_init
struct Bag[T: Copyable & Movable](Iterable):
    comptime Element = Self.T
    var value: Self.T

def consume[C: Iterable](c: C) raises -> C.Element:
    for item in c:
        return item.copy()
    raise "empty"
