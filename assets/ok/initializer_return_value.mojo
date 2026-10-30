# A `return value` in an `out self` initializer initializes the receiver
# with the value, as `self = value; return` does: a trivial and a
# destructible field, and a fall-through path that assigns the fields.
struct P(Copyable):
    var x: Int

    def __init__(out self, *, fill: Int):
        self.x = fill

    def __init__(out self, a: Int, b: Int):
        if a == b:
            return Self(fill=a)
        self.x = a + b


struct Q(Copyable):
    var s: String

    def __init__(out self, *, fill: String):
        self.s = fill

    def __init__(out self, a: String, b: String):
        if a == b:
            return Self(fill=a + "!")
        self.s = a + b


def main():
    print(P(1, 1).x, P(1, 2).x)
    var p = Q("x", "x")
    var q = Q("x", "y")
    print(p.s, q.s)
