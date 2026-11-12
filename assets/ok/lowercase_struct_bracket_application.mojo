# A bracket over a lowercase struct name parses as a subscript (`pair[5, 6]`
# is a `MultiIndex`), yet it is the struct's parameter application wherever
# a compile-time value is read, as upstream classifies one `SubscriptNode`
# by its base: a local `comptime` alias, positional or keyword, and a type
# argument (`show[pair[1, 2]]()`).


@fieldwise_init
struct pair[a: Int, b: Int](Copyable):
    def total(self) -> Int:
        return Self.a + Self.b


def show[T: AnyType]():
    print("ok")


def main():
    show[pair[1, 2]]()
    comptime X = pair[5, 6]
    print(X().total())
    comptime Y = pair[a=1, b=2]
    print(Y().total())
