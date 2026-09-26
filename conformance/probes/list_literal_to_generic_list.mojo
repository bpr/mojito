# A list literal passed where a `List` built over a binder is expected: the
# pin infers `T = Int` for `count([1, 2])` and builds the `Bag[Int]` field
# from `[1, 2]`, printing "2" twice; Mojito reports "cannot infer type
# parameter 'T' of 'count' from the arguments", and the construction alone
# "type mismatch for field 1 of 'Bag': expected List[Int], found List[T]".
@fieldwise_init
struct Bag[T: Copyable & Deinitable](Movable):
    var extra: List[Self.T]


def count[T: Copyable & Deinitable](extra: List[T]) -> Int:
    return len(extra)


def main():
    print(count([1, 2]))
    var bag = Bag[Int]([1, 2])
    print(len(bag.extra))
