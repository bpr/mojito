# A trait constant is typed by its annotation, which widens its metatype:
# `Q` binds `Copyable` as an `AnyTrait[Movable]`, which a `Copyable`-typed
# parameter does not take.
# expect: 'g' parameter 'Tr' has 'AnyTrait[Copyable]' type, but value has type 'AnyTrait[Movable]'
comptime Q: type_of(Movable) = Copyable


def g[Tr: type_of(Copyable)]() -> Int:
    return 1


def main():
    print(g[Q]())
