# A store and an in-place update through a mutable `ref` getter bound by a
# keyword, positional-and-keyword, multi-index, or slice bracket write the
# referent the getter returns.
from std.builtin.builtin_slice import ContiguousSlice


struct Cells:
    var a: Int
    var name: String

    def __init__(out self):
        self.a = 0
        self.name = "x"

    def __getitem__(ref self, *, k: Int) -> ref[self.a] Int:
        return self.a

    def __getitem__(ref self, i: Int, *, label: Bool) -> ref[self.name] String:
        return self.name

    def __getitem__(ref self, i: Int, j: Int) -> ref[self.a] Int:
        return self.a

    def __getitem__(ref self, span: ContiguousSlice) -> ref[self.a] Int:
        return self.a


def main():
    var s = Cells()
    s[k=0] = 5
    s[k=0] += 2
    print(s.a)
    s[0, label=True] = "ab"
    s[0, label=True] += "cd"
    print(s.name)
    s[1, 2] = 10
    s[1, 2] -= 3
    print(s.a)
    s[1:2] = 20
    s[1:2] += 1
    print(s.a)
