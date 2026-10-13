# Pinned against Mojo 1.2.0.dev2026100406 (2026-10-13): each `_ = e`
# statement owns its value alone and destroys it before the next statement,
# whatever the other discards in the function hold.
# Expected: delPair 1 / delPair 1
struct Pair(Movable):
    var a: Int
    var b: Int

    def __init__(out self):
        self.a = 1
        self.b = 2

    def __deinit__(deinit self):
        print("delPair", self.a)


def pair_then_literal():
    var pair = Pair()
    _ = pair^
    _ = 5


def literal_then_pair():
    _ = 5
    var pair = Pair()
    _ = pair^


def main():
    pair_then_literal()
    literal_then_pair()
