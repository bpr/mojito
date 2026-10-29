# A refined `out self` may name a struct parameter only in that parameter's
# own position.
# expect: cannot use Self parameter 'b' in constructor whose result defines it to 'a'
struct P[a: Int, b: Int](Movable):
    var v: Int

    def __init__(out self: P[Self.b, Self.a], *, swap: Int):
        self.v = swap

def main():
    print(P[1, 2](swap=3).v)
