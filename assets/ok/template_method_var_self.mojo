# A `var self` method that transfers its receiver out (`return self^`)
# derives from its checked template: the transfer owes `Movable` at the
# instance's type, and the result is the receiver's own type. The owned `List`
# iterator's `__iter__` is such a method.
@fieldwise_init
struct Box[T: Movable & Deinitable](Movable):
    var item: Self.T
    var uses: Int

    def bumped(var self) -> Self:
        self.uses += 1
        return self^

    def itself(var self) -> Self:
        return self^


def main():
    var a = Box(1, 0).bumped().itself()
    var b = Box(String("x"), 3).bumped()
    print(a.item, a.uses, b.item, b.uses)
    var values: List[Int] = [4, 5]
    for var value in values^:
        print(value)
