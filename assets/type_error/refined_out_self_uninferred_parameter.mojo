# A refined `out self` naming an initializer's own parameter that only its
# default binds leaves the struct parameter uninferred: the default never
# feeds the struct.
# expect: cannot infer type parameter 'n' of 'W'
struct W[n: Int](Movable):
    var v: Int

    def __init__[k: Int = 4](out self: W[k], *, gen: Int):
        self.v = gen + k

def main():
    print(W(gen=1).v)
