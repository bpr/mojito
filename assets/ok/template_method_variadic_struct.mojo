# A user variadic struct specialized whole (`Bag$t2[Int, String]`) binds
# its pack to the element types its trace names, and its template's `Self`
# names the specialization, so `__len__`, `bump` (a sibling call and an
# augmented store), `twice` (a field of a sibling call's result), and the
# synthesized `copy` derive from their checked templates.
struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable, Sized):
    var storage: Tuple[*Self.Ts]
    var count: Int

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)
        self.count = 0

    def __len__(self) -> Int:
        return len(self.storage)

    def bumped(self) -> Self:
        var other = self.copy()
        other.count += 1
        return other^

    def twice(self) -> Int:
        return self.bumped().count + len(self)

    def bump(mut self):
        self.count += self.twice()


def main():
    var b = Bag[Int, String](1, "a")
    b.bump()
    print(b.count, len(b), b.bumped().count)
    var c = Bag[Bool, Int](True, 2)
    c.bump()
    print(c.twice())
