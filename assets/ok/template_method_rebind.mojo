# A `rebind` in a method of a generic struct: `rebind[Int](self.value)`
# under `comptime if Self.T == Int`. The method is served by its template,
# checked once with `T` symbolic: the elaborator selects each instance's arm
# and judges the `rebind`'s equality only in the arm it keeps.
struct Box[T: Movable & Copyable & Deinitable](Movable, Copyable):
    var value: Self.T
    var count: Int

    def __init__(out self, var value: Self.T, count: Int):
        self.value = value^
        self.count = count

    def as_int(self) -> Int:
        comptime if Self.T == Int:
            return rebind[Int](self.value) + self.count
        else:
            return self.count

    # A rebound field written in place.
    def bump(mut self):
        comptime if Self.T == Int:
            rebind[Int](self.value) += self.count


def main():
    var a = Box[Int](5, 2)
    var b = Box[String]("x", 3)
    print(a.as_int(), b.as_int())
    a.bump()
    b.bump()
    print(a.value, b.value)
