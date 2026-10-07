# A reached struct instance instantiates every constructor overload, called
# or not: `S[5](3)` calls only the second `__init__`, yet the first, whose
# `[1, 2][Self.n]` is out of range at `n = 5`, fails with "keeps the
# parameter constant". The pin instantiates only the called one and prints 8.
struct S[n: Int]:
    var x: Int

    def __init__(out self):
        comptime q = [1, 2][Self.n]
        self.x = q

    def __init__(out self, y: Int):
        self.x = y + Self.n


def main():
    print(S[5](3).x)
