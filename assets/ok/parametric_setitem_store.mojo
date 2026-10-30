# A subscript store through a setter generic over its `Indexer` index binds
# the setter as the explicit `__setitem__` call does, whether the getter is
# generic too or takes an `Int`.
struct B(Copyable):
    var x: Int

    def __init__(out self):
        self.x = 0

    def __getitem__[I: Indexer, //](self, idx: I) -> Int:
        return self.x

    def __setitem__[I: Indexer, //](mut self, idx: I, val: Int):
        self.x = val + 2


struct C(Copyable):
    var x: Int

    def __init__(out self):
        self.x = 0

    def __getitem__(self, idx: Int) -> Int:
        return self.x

    def __setitem__[I: Indexer, //](mut self, idx: I, val: Int):
        self.x = val + index(idx)


def main():
    var b = B()
    b[2] = 9
    print(b[0])
    var i = 1
    b[i] = 3
    print(b[UInt8(0)])
    var c = C()
    c[1] = 7
    print(c[0])
