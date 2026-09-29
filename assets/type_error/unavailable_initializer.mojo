# expect: conforms_to(T, Defaultable)
# A constructor whose availability clause fails for the constructed type is
# no candidate, so the call reports the clause.
struct Plain(Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


struct Box[T: Movable](Movable):
    var n: Int

    def __init__(out self) where conforms_to(Self.T, Defaultable):
        self.n = 1

    def __init__(out self, n: Int):
        self.n = n


def main():
    var box = Box[Plain]()
    print(box.n)
