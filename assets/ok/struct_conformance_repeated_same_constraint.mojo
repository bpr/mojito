# A trait may be listed more than once when every entry carries the same
# constraint, compared as upstream does: closed conditions fold, an entry
# without `where` is `True`, a `<` reads as the swapped `>`, and the
# operands of `==` and `and` and the diagnostic message do not count.


@fieldwise_init
struct Pair[n: Int](
    Movable where 1 == 1,
    Movable,
    Copyable where n > 0 and n < 5,
    Copyable where n < 5 and 0 < n,
    Writable where (n == 2, "first"),
    Writable where (2 == n, "second"),
):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Pair(", self.x, ")")


def main():
    var p = Pair[2](7)
    var q = p.copy()
    print(q.x)
    print(p)
