# A `comptime assert conforms_to(X, T)` licenses `T` on `X` for the rest of
# its block, as a `comptime if conforms_to(X, T):` arm does at the pin: on a
# type parameter, through `and`, on a struct's `Self.T`, and on a reflected
# field type at a `comptime for` index.

def show[T: AnyType](x: T):
    comptime assert conforms_to(T, Writable), "show needs Writable"
    print(x)


def show_both[T: AnyType](x: T):
    comptime assert conforms_to(T, Writable) and True
    print(x)


struct Box[T: AnyType]:
    def __init__(out self):
        pass

    def show(self, x: Self.T):
        comptime assert conforms_to(Self.T, Writable), "box"
        print(x)


@fieldwise_init
struct P(Copyable):
    var a: Int
    var b: String


def defaults[T: AnyType]():
    comptime types = reflect[T].field_types()
    comptime for i in range(reflect[T].field_count()):
        comptime assert conforms_to(
            types[i], Defaultable & Deinitable & Writable & Copyable
        ), "field"
        var x = types[i]()
        print("[", x, "]")


def main():
    show(1)
    show_both("two")
    Box[Int]().show(3)
    defaults[P]()
