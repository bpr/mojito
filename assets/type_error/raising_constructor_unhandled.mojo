# expect: requires a surrounding 'try' block
# A construction through a raising `__init__` is a raising call.
struct P(Movable):
    var t: Int

    def __init__(out self, z: Int) raises:
        self.t = z


def f():
    var p = P(1)
    print(p.t)


def main():
    f()
