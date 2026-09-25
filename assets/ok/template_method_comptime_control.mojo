# A compile-time-keyed method of a generic struct: `comptime if` over the
# struct's parameter and `comptime for` over a literal range. Source
# validation checks every arm once with `T` symbolic; each instance keeps the
# arms the elaborator selected, once per unrolled copy, and derives its facts
# from the checked template instead of being checked again.
def bump(x: Int) -> Int:
    return x + 1


struct Box[T: Movable & Copyable & Deinitable](Movable, Copyable):
    var value: Self.T
    var count: Int

    def __init__(out self, var value: Self.T, count: Int):
        self.value = value^
        self.count = count

    # The body opens with the selected arm, so each instance's first
    # statement is its own.
    def kind(self) -> Int:
        comptime if Self.T == Int:
            return 1
        elif Self.T == String:
            return 2
        else:
            return 3

    # A loop variable read beside a runtime operand, and one lent to a
    # read parameter, fold to each copy's literal.
    def total(self) -> Int:
        var acc = 0
        comptime for i in range(3):
            acc += i * self.count
            acc += bump(i)
        return acc

    # A local declared inside the loop is one binding per unrolled copy, and
    # an arm nested in the loop is selected per copy.
    def stepped(self) -> Int:
        var acc = 0
        comptime for i in range(4):
            var step = i
            comptime if i % 2 == 0:
                step += self.count
            else:
                step += 1
            acc += step
        return acc

    def scale(mut self):
        comptime for i in range(2):
            self.count += i

    def describe(self, mut writer: Some[Writer]):
        comptime if Self.T == Int:
            writer.write("int box ")
        else:
            writer.write("box ")
        writer.write(self.count)


def main():
    var a = Box[Int](1, 2)
    var b = Box[String]("x", 3)
    var c = Box[Bool](True, 4)
    print(a.kind(), b.kind(), c.kind())
    print(a.total(), b.total(), c.total())
    print(a.stepped(), b.stepped())
    a.scale()
    b.scale()
    var s = String()
    a.describe(s)
    s += " / "
    b.describe(s)
    print(s)
