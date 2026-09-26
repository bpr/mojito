# A per-instantiation method clone inherits its checked template's facts when
# a read method copies `self` into a `var` local and then reads and writes the
# local's fields (`docs/notes/instantiation-from-template.md`, class
# MethodBody). The copy is a copied place, whose implicit copy an instance owes
# again at its own type; a field of the local has its declared type under the
# local's recorded arguments, so a scalar store, a whole-value store, and a
# field read derive as they do on a writable `self`. `Span`'s contiguous slice
# stores a pointer offset into such a local's field.
@fieldwise_init
struct Window[T: ImplicitlyCopyable & Deinitable](
    Deinitable, ImplicitlyCopyable, Movable
):
    var value: Self.T
    var start: Int
    var size: Int

    def shifted(self, by: Int) -> Self:
        var result = self
        result.start = result.start + by
        result.size = self.size - by
        return result^

    def replaced(self, var value: Self.T) -> Self:
        var result = self
        result.value = value^
        return result^

    def trimmed(self) -> Self:
        var result = self
        result.size -= 1
        return result^


def main():
    var numbers = Window[Int](1, 0, 3)
    var words = Window[String]("a", 0, 2)
    var n = numbers.shifted(1)
    var w = words.shifted(1)
    print(n.start, n.size, w.start, w.size)
    print(numbers.replaced(5).value, words.replaced("q").value)
    print(numbers.trimmed().size, words.trimmed().size)
    var xs: List[Int] = [1, 2, 3, 4]
    var ys: Span[Int, origin_of(xs)] = xs
    var middle = ys[1:3]
    var ws: List[String] = ["a", "b", "c"]
    var vs: Span[String, origin_of(ws)] = ws
    var tail = vs[1:]
    print(len(middle), middle[0], middle[1], len(tail), tail[0])
