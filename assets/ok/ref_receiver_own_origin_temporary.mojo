# A method's own origin binder its `ref[o] self` clause names binds from
# the receiver, argument 0, whatever the receiver is: a place, a temporary
# (its materialized slot, lent immutably), a dereferenced pointer, and the
# explicit `S.m(x)` form. A free `ref[o]` parameter binds from a temporary
# argument the same way.


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def __init__(out self, x: Int):
        self.x = x

    def peek[o: Origin](ref[o] self) -> Int:
        return self.x

    def first[o: Origin](ref[o] self) -> ref[o] Self:
        return self


def by[o: Origin](ref[o] s: S) -> ref[o] S:
    return s


def main():
    var s = S()
    var p = Pointer(to=s)
    print(S(2).peek(), p[].peek(), S.peek(S(3)))
    print(S(4).first().x, by(S(5)).x)
    p[].first().x = 6
    print(s.x, by(s).x)
