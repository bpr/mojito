# `NoneType` is the `std.builtin.none` struct over the `!kgen.none` type of
# the `None` expression: `None` converts to it through its `@implicit`
# initializer, and it is `Writable`, as at the pin.


@fieldwise_init
struct Holder(Copyable):
    var n: NoneType


def take(x: NoneType):
    print(x)


def show[T: Writable](x: T):
    print(x)


def main():
    var x: NoneType = None
    print(x)
    take(None)
    print(NoneType())
    show(NoneType())
    print(String(NoneType()), repr(NoneType()))
    var s = String()
    x.write_to(s)
    x.write_repr_to(s)
    print(s, s.byte_length())
    print(Holder(None).n)
    comptime assert NoneType != type_of(None), "two types"
    var o: Optional[Int] = None
    var p = Optional[Int](None)
    print(o.__bool__(), p.__bool__(), o is None)
