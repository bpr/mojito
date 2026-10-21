# expect: cannot call raising function in boolean condition
# A `comptime match` test the elaborator evaluates may not call a raising
# `__eq__`.
@fieldwise_init
struct R(Copyable, ImplicitlyCopyable):
    var a: Int

    def __eq__(self, other: Int) raises -> Bool:
        return self.a == other


def main() raises:
    comptime r = R(1)
    comptime match r:
        case 1:
            print("one")
        case _:
            print("other")
