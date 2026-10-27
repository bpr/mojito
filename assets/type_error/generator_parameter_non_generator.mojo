# A generator-typed parameter takes a generator, not a value.
# expect: invalid call to 'apply': 'apply' parameter 'G' has '__generator_type[Idx: Int] Int' type, but value has type
def apply[G: __generator_type[Idx: Int] Int]() -> Int:
    return G[3]


def main():
    print(apply[5]())
