# A method holding an unkeyed `rebind` (`rebind[Int](self.value)`) is checked
# for every instance of its struct, called or not, so `Box[String]` is
# refused with "type mismatch for rebind: ... expected Int, found String".
# The pin judges only the instances a call reaches and prints "5 x".
struct Box[T: Copyable & Deinitable](Movable, Copyable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def get_int(self) -> Int:
        return rebind[Int](self.value)


def main():
    var a = Box[Int](5)
    var b = Box[String]("x")
    print(a.get_int(), b.value)
