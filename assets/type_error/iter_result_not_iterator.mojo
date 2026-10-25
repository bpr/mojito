# expect: __iter__ return type
# A `for` loop calls `__iter__` once, as upstream's `emitForStmt` does, so an
# `__iter__` whose result is not itself an iterator is rejected even when that
# result has an `__iter__` of its own.
struct It(Iterator, Movable):
    comptime Element = Int
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __next__(mut self) raises StopIteration -> Int:
        if self.n == 0:
            raise StopIteration()
        self.n -= 1
        return self.n


struct Mid(Movable):
    var s: String
    var n: Int

    def __init__(out self, n: Int):
        self.s = String("mid")
        self.n = n

    def __iter__(self) -> It:
        return It(self.n)


struct Outer(Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __iter__(ref self) -> Mid:
        return Mid(self.n)


def main():
    var t = 0
    for var x in Outer(3):
        t += x
    print(t)
