# Lifecycle copy/move: a pointer-owning struct defines unified `__init__`
# overloads for copy (deep-copy the buffer) and move (relocate). Copyable
# conformance plus the copy initializer gives the type value semantics.
# would alias the buffer — the wrong value semantics.
struct Buf:
    var data: UnsafePointer[Int]
    var n: Int

    def __init__(out self, n: Int):
        self.data = UnsafePointer[Int].alloc(n)
        self.n = n
        var i: Int = 0
        while i < n:
            self.data[i] = 0
            i = i + 1

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        self.data = UnsafePointer[Int].alloc(copy.n)
        var i: Int = 0
        while i < copy.n:
            self.data[i] = copy.data[i]
            i = i + 1

    def __init__(out self, *, move: Self):
        self.n = move.n
        self.data = move.data

    def set(mut self, i: Int, v: Int):
        self.data[i] = v

    def get(self, i: Int) -> Int:
        return self.data[i]

def main():
    var a: Buf = Buf(2)
    a.set(0, 100)
    var b: Buf = a           # copy initializer → independent buffer
    b.set(0, 999)
    print(a.get(0), b.get(0))
    var c: Buf = b^          # move initializer → relocate b into c
    print(c.get(0))
