# A struct whose origin has a `mut=` binder has one instance per closed
# mutability: `Q[True, o]` and `Q[False, o]` are two types, and a member
# reading the origin's mutability answers each its own.


struct Q[mut: Bool, //, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def mutable(self) -> Bool:
        return Self.origin.mut

    def get(self) -> Int:
        return self.n


def total(s: Span[Int, _]) -> Int:
    return len(s) + s[0]


def main():
    var x = 1
    print(Q[origin_of(x)](4).mutable())
    print(Q[ImmutAnyOrigin](5).mutable())
    print(Q[MutAnyOrigin](6).mutable())
    print(Q[origin_of(x)](4).get(), Q[ImmutAnyOrigin](5).get())
    var xs = [1, 2, 3]
    var m = Span(xs)
    var i = Span[Int, ImmutAnyOrigin](xs)
    print(len(m), len(i), m[0], i[2])
    print(total(m), total(i))
    _ = xs^
