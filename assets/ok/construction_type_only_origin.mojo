# A construction whose result type alone names an origin,
# `V[origin_of(a, b)](7)`, keeps both members alive until the value's last
# use, though no argument lent either: a value keeps the storage its type's
# origins name live.
struct D(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)


struct V[o: Origin](Movable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k


def main():
    var a = D(1)
    var b = D(2)
    var v = V[origin_of(a, b)](7)
    print("use")
    print(v.k)
    print("end")
