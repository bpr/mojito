# The builtin `iter(iterable)`: the borrowed overload borrows an `Iterable`
# at the argument's origin, the owned one consumes an `IterableOwned`
# (`Some[IterableOwned]`, whose result names `type_of(iterable)`).
def own_iterator(var source: Some[IterableOwned]) -> type_of(source).IteratorOwnedType:
    return source^.__iter__()


@fieldwise_init
struct Sources:
    def owned(self, var s: Some[IterableOwned]) -> type_of(s).IteratorOwnedType:
        return s^.__iter__()

    @staticmethod
    def static_owned(
        var s: Some[IterableOwned],
    ) -> type_of(s).IteratorOwnedType:
        return s^.__iter__()


def main() raises:
    var xs: List[Int] = [4, 5, 6]
    var it = iter(xs)
    print(next(it), next(it))
    var lower, _ = iter(xs).bounds()
    print(lower)
    for x in iter(xs):
        print(x)
    var d: Dict[String, Int] = {"a": 1, "b": 2}
    for k in iter(d):
        print(k)
    var owned = iter(xs.copy())
    print(next(owned))
    var consumed = own_iterator(xs^)
    print(next(consumed), next(consumed))
    var by_method = Sources().owned([7, 8])
    print(next(by_method))
    var by_static = Sources.static_owned([9])
    print(next(by_static))
    var s = "hi"
    for c in iter(s.codepoints()):
        print(c)
