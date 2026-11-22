# A plain store through a method's mutable `ref` result writes the referent
# when the call binds a generic parameter by keyword, as it does for a
# non-generic keyword call or a generic positional one.


struct S:
    var a: Int

    def __init__(out self):
        self.a = 0

    def get[I: Indexer](ref self, *, k: I) -> ref[self.a] Int:
        return self.a

    def get2(ref self, *, k: Int) -> ref[self.a] Int:
        return self.a

    def get4[I: Indexer](ref self, k: I) -> ref[self.a] Int:
        return self.a


def main():
    var s = S()
    s.get(k=0) = 6
    print(s.a)
    s.get2(k=0) = 5
    print(s.a)
    s.get4(0) = 4
    print(s.a)
    s.get(k=0) += 6
    print(s.a)
    var l = [1, 2, 3]
    var p = l.unsafe_ptr()
    p.__getitem__(unsafe_offset=1) = 7
    print(l[1])
