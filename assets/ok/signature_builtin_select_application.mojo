# A builtin callee generic over a type folds in a signature: its body is the
# `pop.select` over the call's operands, so `Tag[select(1 > 2, 1, 0)]` is
# `Tag[0]`.
from std.utils._select import _select_register_value


struct Tag[k: Int]:
    def __init__(out self):
        pass

    def key(self) -> Int:
        return Self.k


def chosen[n: Int](t: Tag[_select_register_value(n > 2, n, 0)]):
    print(n, t.key())


def main():
    chosen[1](Tag[0]())
    chosen[3](Tag[3]())
