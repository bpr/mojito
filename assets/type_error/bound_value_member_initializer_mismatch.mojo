# An initializer's parameter reading a bound's value member (`Box[Self.T.K]`)
# closes at the instance's conformer, so a `Box` of another size is refused.
# expect: type mismatch for argument 1 to 'W.__init__': expected Box[3], found Box[4]
trait Tr:
    comptime K: Int


struct S(Tr):
    comptime K: Int = 3


struct Box[n: Int](Movable):
    def __init__(out self):
        pass


struct W[T: Tr](Movable):
    var b: Box[Self.T.K]

    def __init__(out self, var b: Box[Self.T.K]):
        self.b = b^


def main():
    _ = W[S](Box[4]())
