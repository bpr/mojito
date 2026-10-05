# A generic struct's method holding a `rebind` is served by its template:
# the elaborator judges the rebind's type equality only for the instances a
# call reaches, so `Box[String]`, whose `get_int` is never called, runs. A
# rebound place is read, written in place, and assigned whole.
struct Box[T: Copyable & Deinitable](Movable, Copyable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def get_int(self) -> Int:
        return rebind[Int](self.value)

    def bump(mut self, n: Int):
        rebind[Int](self.value) += n

    def reset(mut self):
        rebind[Int](self.value) = 0


def as_int[T: Copyable & Deinitable](x: T) -> Int:
    var y = x.copy()
    return rebind[Int](y)


def main():
    var a = Box[Int](5)
    var b = Box[String]("x")
    print(a.get_int(), b.value)
    a.bump(3)
    print(a.get_int(), as_int(a.value))
    a.reset()
    print(a.value)
