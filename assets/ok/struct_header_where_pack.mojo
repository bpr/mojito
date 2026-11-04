# A struct header's `where` clause compiles with the header's binders in
# scope: a pack's length, its `all_conforms_to`, and a module constant.
comptime N = 2


struct Pair[*Ts: Movable] where Ts.length == 2:
    @staticmethod
    def two() -> Int:
        return 2


struct Copies[*Ts: Movable] where Ts.all_conforms_to[Copyable]():
    @staticmethod
    def count() -> Int:
        return 3


struct Sized[*Ts: Movable] where Ts.length == N:
    @staticmethod
    def size() -> Int:
        return N


def main():
    print(Pair[Int, Bool].two())
    print(Copies[Int, String].count())
    print(Sized[Int, Bool].size())
