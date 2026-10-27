# A generator converts to a generator-typed parameter only with the same
# parameter list: a two-parameter alias is no `__generator_type[Idx: Int]
# Int`.
# expect: 'apply' parameter 'G' has '__generator_type[Idx: Int] Int' type, but value has type '__generator_type[a: Int, b: Int] Int'
comptime Add[a: Int, b: Int] = a + b


def apply[G: __generator_type[Idx: Int] Int]() -> Int:
    return G[3]


def main():
    print(apply[Add]())
