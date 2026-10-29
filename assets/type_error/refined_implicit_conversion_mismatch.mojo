# An `@implicit` initializer converts only into the type its refined
# `out self` defines.
# expect: expected W[3], found Bool
struct W[n: Int](Movable):
    var v: Int

    @implicit
    def __init__(out self: W[2], flag: Bool):
        self.v = Int(flag)

def take_three(w: W[3]):
    print(w.v)

def main():
    take_three(True)
