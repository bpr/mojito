# expect: rebind input type 'String' does not match result type 'Int'
# A method's `rebind` is judged per instance a call reaches: `get_int` holds
# for `Box[Int]` and fails for `Box[String]`, which the pin reports as the
# instantiation failing.
struct Box[T: Copyable & Deinitable](Movable, Copyable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def get_int(self) -> Int:
        return rebind[Int](self.value)


def main():
    print(Box[Int](5).get_int())
    print(Box[String]("x").get_int())
