# A struct parameter's default names a module alias that constructs a
# struct of the same program: the alias waits for `Key`'s initializers,
# and `Box`'s default for the alias.
@fieldwise_init
struct Key(ImplicitlyCopyable, Movable):
    var v: Int


struct Holder[k: Key](Defaultable):
    def __init__(out self):
        pass

    def key(self) -> Int:
        return Self.k.v


comptime DefaultHolder = Holder[Key(7)]


struct Box[H: Defaultable = DefaultHolder]:
    def __init__(out self):
        pass


def main():
    _ = Box()
    var h = DefaultHolder()
    print(h.key())
