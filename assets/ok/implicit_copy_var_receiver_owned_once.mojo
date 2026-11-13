# A `var self` method called on a named `ImplicitlyCopyable` value whose
# field owns storage takes an implicit copy as its owned argument: the
# callee destroys the copy, and the caller destroys only the original, at
# its last use (where the copy is a move), never the copy again.
from std.collections import OptionalReg


struct W(ImplicitlyCopyable, Movable):
    var o: Optional[Int]

    def __init__(out self, n: Int):
        self.o = Optional[Int](n)

    def or_else(var self, var d: Int) -> Int:
        if self.o:
            return self.o.value()
        return d


def main():
    var w = W(7)
    print(w.or_else(1))
    print(w.or_else(2))
    var e = OptionalReg[Int]()
    print(e.or_else(3))
