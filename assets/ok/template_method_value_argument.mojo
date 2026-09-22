# A per-instantiation method clone inherits its checked template's facts when
# the body hands a whole value of the struct's parameter type to a by-value
# parameter of a method call (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `value_arguments`): a `^` transfer, a sibling
# call's result, a place the template already copied, or a place a read
# parameter takes where it lies. The argument's type is exactly the
# parameter's, so nothing converts it under any instance, and what the call
# records for it is decided by its syntax and the callee's conventions. The
# callee may belong to a field of another struct, whose parameter types were
# recorded at the field's own arguments.
struct Slot[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def put(mut self, var value: Self.T):
        self.item = value^


struct Bag[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var slot: Slot[Self.T]
    var n: Int

    def __init__(out self, var item: Self.T):
        self.item = item.copy()
        self.slot = Slot[Self.T](item^)
        self.n = 0

    def find(self, key: Self.T) -> Int:
        return self.n

    def has(self, key: Self.T) -> Bool:
        return self.find(key) >= 0

    def put(mut self, var value: Self.T):
        self.item = value^
        self.n += 1

    def put_moved(mut self, var value: Self.T):
        self.put(value^)

    def store(mut self, var value: Self.T):
        self.slot.put(value^)

    def make(self) -> Self.T:
        return self.item.copy()

    def put_temp(mut self):
        self.put(self.make())

    def has_own(self) -> Bool:
        return self.has(self.item)

    def has_temp(self) -> Bool:
        return self.has(self.make())


struct Cell[T: ImplicitlyCopyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def put(mut self, var value: Self.T):
        self.item = value^

    def put_copy(mut self, value: Self.T):
        self.put(value)

    def put_own(mut self):
        self.put(self.item)


def main():
    var a = Bag[Int](1)
    var b = Bag[String](String("x"))
    a.put_moved(2)
    a.put_temp()
    a.store(8)
    b.put_moved(String("y"))
    b.put_temp()
    b.store(String("s"))
    print(a.has(3), b.has(String("z")), a.has_own(), b.has_own())
    print(a.has_temp(), b.has_temp(), a.n, b.n, a.item, b.item)
    print(a.slot.item, b.slot.item)
    var d = Cell[Int](1)
    d.put_copy(4)
    d.put_own()
    print(d.item)
