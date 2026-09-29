# expect: Defaultable
# `Tuple`'s default initializer is available only where every element is
# `Defaultable`.
struct Plain(Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def main():
    var pair = Tuple[Plain, Int]()
    print(len(pair))
