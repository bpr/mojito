# A copy of a slot that dies right at the copy is the slot's move, as
# upstream's lifetime check turns a copy initialization followed by the
# source's destruction into a move: binding it, passing it to a `var`
# parameter, and building a tuple from it run no copy initializer, and a
# `read` argument borrows the slot in place.
struct Counted(ImplicitlyCopyable, Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __init__(out self, *, copy: Self):
        self.tag = copy.tag + 100
        print("copy", self.tag)

    def __deinit__(deinit self):
        print("del", self.tag)


def take(var c: Counted):
    print("take", c.tag)


def look(c: Counted):
    print("look", c.tag)


def main():
    var c = Counted(1)
    var d = c
    print("d", d.tag)
    var e = Counted(2)
    take(e)
    print("mid")
    var f = Counted(3)
    var t = (f, 5)
    print("t", t[0].tag)
    var g = Counted(4)
    look(g)
    print("end")
