# A per-instantiation method clone inherits its checked template's facts when
# the body binds an annotated `var` local (`docs/notes/instantiation-from-template.md`,
# class MethodBody, obligation Implicit conversions). A value that converts to
# the declared type is recorded as a conversion at the value, which the
# instance selects again from its own types: a literal reaches a constructor
# of a target built over the struct's parameter, and a field of `self` of the
# parameter type reaches the instance's own constructor clone, which reads it
# in place. An annotation equal to the value's type converts nothing, and a
# scalar annotation keeps its local a scalar.
struct Label[T: Copyable & Deinitable](Copyable, Deinitable, Movable):
    var count: Int

    @implicit
    def __init__(out self, count: Int):
        self.count = count

    def size(self) -> Int:
        return self.count


struct Wrapper[T: Copyable & Deinitable](Copyable, Deinitable, Movable):
    var value: Self.T

    @implicit
    def __init__(out self, value: Self.T):
        self.value = value.copy()


struct Holder[T: Copyable & Deinitable](Deinitable, Movable):
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.items = List[Self.T]()
        self.items.append(item.copy())
        self.item = item^

    def labeled(self) -> Label[Self.T]:
        var label: Label[Self.T] = 4
        return label^

    def counted(self) -> Int:
        var label: Label[Self.T] = 6
        return label.size()

    def boxed(self) -> Wrapper[Self.T]:
        var box: Wrapper[Self.T] = self.item
        return box^

    def listed(self) -> Int:
        var items: List[Self.T] = List[Self.T]()
        return len(items)

    def moved(self, var value: Self.T) -> Self.T:
        var kept: Self.T = value^
        return kept^

    def floated(self) -> Float64:
        var ratio: Float64 = 4
        return ratio


def main():
    var number = Holder[Int](5)
    var text = Holder[String](String("hi"))
    print(number.labeled().count, number.counted(), number.boxed().value, number.listed())
    print(text.labeled().count, text.counted(), text.boxed().value, text.listed())
    print(number.moved(3), text.moved("m"), number.floated(), text.floated())
