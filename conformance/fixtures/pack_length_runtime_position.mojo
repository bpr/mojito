# A def's own pack queried in a runtime position: `Us.length` in a return
# expression, of a method's own pack and of a free def's. Both print as the
# pinned Mojo does (4 and 5).
struct Plain:
    var n: Int

    def __init__(out self):
        self.n = 1

    def tally[*Us: Movable](self, var *extra: *Us) -> Int:
        return self.n + Us.length


def count[*Us: Movable](var *extra: *Us) -> Int:
    return 2 + Us.length


def main():
    print(Plain().tally(7, "x", False))
    print(count(7, "x", False))
