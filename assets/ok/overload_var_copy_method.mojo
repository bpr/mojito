# A method overload set ranks as a free function's does. The implicit copy a
# place costs a `var` parameter outranks the specificity tie-break, so `x.m(s)`
# selects the generic overload while the rvalue `x.m(String("t"))` selects the
# `var` one. With no copy in play the specificity tie-break decides, and the
# concrete candidate wins.
# requires: discovery


struct S:
    var v: Int

    def __init__(out self):
        self.v = 0

    def m(self, var a: String) -> Int:
        return 1

    def m[T: Writable](self, a: T) -> Int:
        return 2

    def n(self, a: String) -> Int:
        return 3

    def n[T: Writable](self, a: T) -> Int:
        return 4


def main():
    var s = String("s")
    var x = S()
    print(x.m(s))
    print(x.m(String("t")))
    print(x.n(s))
