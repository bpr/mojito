# The struct header keeps the bare pack name, because `Self` is not available
# there: the conformance clauses and the trailing `where` name `Ts`. Every
# member spells `Self.Ts` — the field, the pack constructor, an availability
# clause, and a body — and a method's own pack (`*Us`) stays bare, since it is
# the method's parameter, not the struct's.
struct Bag[*Ts: Movable](
    Copyable where Ts.all_conforms_to[Copyable](),
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
) where (conforms_to(Ts.values, Movable), "pack elements must be Movable"):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def count(self) -> Int where Self.Ts.all_conforms_to[Copyable]():
        return Self.Ts.length

    def tally[*Us: Movable](self, var *extra: *Us) -> Int:
        var total = Self.Ts.length
        comptime for i in range(Us.length):
            total += 1
        return total


def main():
    var b = Bag[Int, Bool](1, True)
    print(b.count())
    print(b.tally(7, "x", False))
