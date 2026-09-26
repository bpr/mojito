# A per-call method clone (`echo[String]` on `Box[Int]`) bakes the method's
# own parameters as well as the struct's. Its trace names both, so a clone of
# a checked template inherits the template's facts with the struct's and the
# method's binders substituted, instead of being checked again. A
# non-generic struct's per-call clone substitutes the method's binders alone.
struct Box[T: Copyable & Deinitable](Movable, Copyable):
    var value: Self.T
    var count: Int

    def __init__(out self, var value: Self.T, count: Int):
        self.value = value^
        self.count = count

    def tally[U: Copyable & Deinitable](self, other: U) -> Int:
        return self.count + 1

    def echo[U: Copyable & Deinitable](self, other: U) -> U:
        return other.copy()


struct Plain:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def echo[U: Copyable & Deinitable](self, other: U) -> U:
        return other.copy()


def main():
    var b = Box[Int](3, 4)
    var s = Box[String](String("v"), 2)
    print(b.tally[String]("s"), s.tally(1.5))
    print(b.echo[String]("e"), b.echo(7), s.echo(String("w")))
    var p = Plain(7)
    print(p.echo[String]("q"), p.echo(1.5))
