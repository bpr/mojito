# A generator type names the type its application yields after its
# parameter list.
# expect: Expected an expression
def apply[G: __generator_type[Idx: Int]]() -> Int:
    return G[3]


def main():
    pass
