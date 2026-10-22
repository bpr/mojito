# A `@fieldwise_init` constructor takes each field that is not
# register-passable-trivial as an owned `var` argument and moves it into its
# field, as upstream's synthesized fieldwise `__init__` does: a copied place
# argument is copied in the caller and then moved in, a temporary is moved
# in, and a transferred place is moved in once.


struct C(ImplicitlyCopyable, Movable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def __init__(out self, *, copy: Self):
        print("copy")
        self.k = copy.k + 10

    def __init__(out self, *, deinit move: Self):
        print("move")
        self.k = move.k + 100


@fieldwise_init
struct W(Movable):
    var c: C
    var n: Int


def main():
    var c = C(1)
    print("build")
    var w = W(c, 2)
    print(w.c.k, w.n)
    print("temp")
    var w2 = W(C(5), 3)
    print(w2.c.k)
    print("moved")
    var w3 = W(c^, 4)
    print(w3.c.k)
