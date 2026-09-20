# A `where` clause that stays residual needs evidence at the application: the
# enclosing declaration's own clause stating the same proposition, in either
# operand order. A struct's value parameter takes part through `Self.n`.
# (Evidence by canonical identity, `successor[n, 1 + n]()`, is pinned by the
# checker test `param_expr_residual_is_not_false`: natively a value argument
# computed from a caller's parameter is not yet a constant.)
def successor[n: Int, m: Int]() -> Int where n + 1 == m:
    return m


def below[n: Int, m: Int]() -> Int where n < m:
    return m


def by_assumption[n: Int, k: Int]() -> Int where n < k:
    return below[n, k]()


def by_reordered_assumption[n: Int, k: Int]() -> Int where k == n + 1:
    return successor[n, k]()


struct Box[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self):
        self.v = Self.n

    def grown[k: Int](self) -> Int where Self.n + k == 5:
        return k


def main():
    print(by_assumption[3, 9]())
    print(by_reordered_assumption[3, 4]())
    print(Box[3]().grown[2]())
