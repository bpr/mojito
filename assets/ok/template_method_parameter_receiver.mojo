# A per-instantiation method clone inherits its checked template's facts when
# its body calls a method on a read parameter (`docs/notes/
# instantiation-from-template.md`, class MethodBody): alone, as an operator's
# operand, bound to a local, and on a field of the parameter. The parameter is
# bound to the instance's argument and read where it lies, as `self` is.
struct Box[T: Copyable & Equatable & Deinitable](Copyable, Movable):
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.items = List[Self.T]()
        self.items.append(item.copy())
        self.item = item^

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def head(self) -> Self.T:
        return self.item.copy()

    def size(self) -> Int:
        return len(self.items)

    def same(self, value: Box[Self.T]) -> Bool:
        return self.item == value.head()

    def grab(self, value: Box[Self.T]) -> Self.T:
        return value.head()

    def total(self, value: Self) -> Int:
        var n = value.size()
        return n + self.size()

    def count(self, value: Box[Self.T]) -> Int:
        return value.items.__len__()

    def matches(self, value: Box[Self.T], x: Self.T) -> Bool:
        return value.head() == x

    def fill(self, mut into: Box[Self.T]) -> Int:
        into.push(self.head())
        return into.size()

    def owned(self, var value: Box[Self.T]) -> Int:
        value.push(self.head())
        return value.size() + value.items.__len__()


def main():
    var a = Box[Int](3)
    var b = Box[String]("x")
    print(a.same(Box[Int](3)), b.same(Box[String]("y")))
    print(a.grab(Box[Int](4)), b.grab(Box[String]("z")))
    print(a.total(Box[Int](1)), b.total(Box[String]("w")))
    print(a.count(Box[Int](1)), b.count(Box[String]("w")))
    print(a.matches(Box[Int](4), 4), b.matches(Box[String]("q"), "r"))
    var c = Box[Int](7)
    var d = Box[String]("v")
    print(a.fill(c), b.fill(d), c.size(), d.size())
    print(a.owned(Box[Int](1)), b.owned(Box[String]("u")))
