# A `Tuple` move runs each element's move initializer, as upstream's move
# initializer moves every element with `unsafe_write_move_from`; the pack
# initializer moves its argument in and the copy initializer copies each
# element.
struct M(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 1000
        print("copy", self.v)
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1
        print("move", self.v)
    def __deinit__(deinit self):
        print("del", self.v)

def main():
    var m = M(1)
    print("-- pack")
    var t = (m^, 3)
    print("-- copy")
    var u = t.copy()
    print("-- move")
    var w = t^
    print("-- end", u[0].v, w[0].v)
