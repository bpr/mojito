# A module alias that a struct parameter's default names reports its own
# construction's rejection, not an unknown type.
@fieldwise_init
struct Key(ImplicitlyCopyable, Movable):
    var v: Int


struct Holder[k: Key](Defaultable):
    def __init__(out self):
        pass


comptime DefaultHolder = Holder[Key("x")]


struct Box[H: Defaultable = DefaultHolder]:
    def __init__(out self):
        pass


def main():
    _ = Box()
