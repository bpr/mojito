# A def's own pack queried in a runtime position: `Us.length` in a return
# expression. The pinned Mojo runs it (prints 4 and 5); Mojito folds a pack's
# `length` only inside a variadic struct's methods and rejects this with
# "Undefined variable 'Us'". Tracked as `pack-length-runtime-position` in
# `docs/roadmap.md`.
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
