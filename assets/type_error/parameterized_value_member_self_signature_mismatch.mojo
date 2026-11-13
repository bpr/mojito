# expect: struct 'S' method 'take' does not match the signature required by trait 'Tr'
# A trait method's `Box[Self.get[0]]` is the conformer's member bound to
# `0`, so a conformer taking another `Box` does not implement it.


struct Box[n: Int]:
    def __init__(out self):
        pass


trait Tr:
    comptime get[idx: Int]: Int

    def take(self, b: Box[Self.get[0]]) -> Int:
        ...


struct S(Tr):
    comptime get[idx: Int]: Int = idx + 1

    def __init__(out self):
        pass

    def take(self, b: Box[2]) -> Int:
        return b.n


def main():
    print(S().take(Box[2]()))
