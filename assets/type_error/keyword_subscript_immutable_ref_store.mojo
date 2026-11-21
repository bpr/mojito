# expect: expression must be mutable in assignment
# A keyword `__getitem__` taking a read `self` returns an immutable
# reference, which a store cannot write through.
struct Cells:
    var a: Int

    def __init__(out self):
        self.a = 0

    def __getitem__(self, *, k: Int) -> ref[self.a] Int:
        return self.a


def main():
    var s = Cells()
    s[k=0] = 5
    print(s.a)
