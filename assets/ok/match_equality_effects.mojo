# An explicit `== expr` pattern evaluates its expression and calls `__eq__`
# once per test, in case order, short-circuiting after a failed tuple
# element; `{v=2}` compares a field.
struct Key(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __eq__(self, other: Self) -> Bool:
        print("eq", self.v, other.v)
        return self.v == other.v


def expected(v: Int) -> Key:
    print("expected", v)
    return Key(v)


def main():
    var k = Key(2)
    match k:
        case == expected(1):
            print("one")
        case == expected(2):
            print("two")
        case _:
            print("other")
    match (k, k):
        case (== expected(1), == expected(2)):
            print("1,2")
        case (== expected(2), == expected(1)):
            print("2,1")
        case (== expected(2), == expected(2)):
            print("2,2")
    match k:
        case {v=1}:
            print("field one")
        case {v=2}:
            print("field two")
