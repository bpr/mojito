# Generic types nested several levels deep: a user `Box` wrapped six times,
# a `Pair` whose `swap` instantiates the transposed pair, and bundled
# collections holding user generics inside `List`, `Dict`, and `Optional`.


@fieldwise_init
struct Box[T: Copyable & Movable & Deinitable](Copyable, Movable):
    var item: Self.T

    def get(self) -> Self.T:
        return self.item.copy()


@fieldwise_init
struct Pair[
    A: Copyable & Movable & Deinitable, B: Copyable & Movable & Deinitable
](Copyable, Movable):
    var first: Self.A
    var second: Self.B

    def swap(self) -> Pair[Self.B, Self.A]:
        return Pair[Self.B, Self.A](self.second.copy(), self.first.copy())


def wrap[T: Copyable & Movable & Deinitable](x: T) -> Box[T]:
    return Box[T](x.copy())


def main() raises:
    var b1 = wrap(1)
    var b2 = wrap(b1.copy())
    var b3 = wrap(b2.copy())
    var b4 = wrap(b3.copy())
    var b5 = wrap(b4.copy())
    var b6 = wrap(b5.copy())
    print(b6.get().get().get().get().get().get())

    var p = Pair[Box[Int], List[Box[String]]](wrap(2), List[Box[String]]())
    p.second.append(wrap(String("deep")))
    var q = p.swap()
    print(q.first[0].get(), q.second.get())

    var grid = List[List[Pair[Int, Box[Float64]]]]()
    var row = List[Pair[Int, Box[Float64]]]()
    row.append(Pair[Int, Box[Float64]](3, wrap(0.5)))
    grid.append(row^)
    print(grid[0][0].first, grid[0][0].second.get())

    var od = Optional[Dict[String, List[Box[Int]]]](
        Dict[String, List[Box[Int]]]()
    )
    od.value()[String("k")] = [wrap(5), wrap(6)]
    print(len(od.value()[String("k")]), od.value()[String("k")][1].get())
