# A parenthesized tuple inside a bracket is one operand, not one per
# element: it binds the first parameter, whose type it does not match.
# expect: type mismatch for value parameter 'a': expected Int, found Tuple[Int, Int]
struct G[a: Int, b: Int]:
    @staticmethod
    def total() -> Int:
        return Self.a + Self.b


def main():
    print(G[(1, 2)].total())
