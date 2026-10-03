# A generic module `def` spelling a static call's receiver with its own type
# parameter, `Pair[T].keep(...)`: the template serves the call, and the
# elaborator binds the receiver from the recorded type. The same spelling
# inside a generic struct's method, `Pair[Self.T].keep(...)`, runs alike.
@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T
    var b: Self.T

    @staticmethod
    def keep(var v: Self.T) -> Pair[Self.T]:
        var w = v.copy()
        return Pair[Self.T](v^, w^)


def make[T: Copyable & Deinitable](x: T) -> Pair[T]:
    return Pair[T].keep(x.copy())


def main():
    print(make(String("s")).a)
