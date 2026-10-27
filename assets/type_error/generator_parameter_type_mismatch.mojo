# A generator's parameter types must match the generator-typed parameter's,
# position by position: a `Bool` parameter is no `Int` one.
# expect: invalid call to 'apply': 'apply' parameter 'G' has '__generator_type[Idx: Int] Int' type, but value has type '__generator_type[a: Bool]
comptime Flag[a: Bool] = 1


def apply[G: __generator_type[Idx: Int] Int]() -> Int:
    return G[3]


def main():
    print(apply[Flag]())
