# `for var value in values^` over a `var *values: Self.T` pack moves each
# element out, so Mojito runs this and prints "2". The pin iterates the
# pack by copy and rejects it: "value of type 'T' cannot be implicitly
# copied, it does not conform to 'ImplicitlyCopyable'". Over an `Int` pack
# both run. The bundled `List`, `Set`, and `Array` literal initializers use
# this spelling.
struct Bag[T: Movable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var *values: Self.T):
        self.items = List[Self.T]()
        for var value in values^:
            self.items.append(value^)


def main():
    var b = Bag(String("x"), String("y"))
    print(len(b.items))
