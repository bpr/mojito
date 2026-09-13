# A free function overloads on a read versus an owned (`var`) parameter of
# the same type. A place argument selects the read overload; an explicit
# transfer `x^` or an rvalue selects the owned one, at the parameter where the
# overloads differ.

struct Thing(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

@fieldwise_init
struct Cop(Copyable, Movable):
    var n: Int

def f(t: Thing):
    print("read", t.n)

def f(var t: Thing):
    print("owned", t.n)

def g(t: Cop):
    print("gread", t.n)

def g(var t: Cop):
    print("gowned", t.n)

def h(x: Int, t: Cop):
    print("hread", x, t.n)

def h(x: Int, var t: Cop):
    print("howned", x, t.n)

def main():
    var a = Thing(1)
    f(a)
    f(a^)
    f(Thing(2))
    var c = Cop(3)
    g(c)
    g(c^)
    g(Cop(4))
    var d = Cop(5)
    h(1, d)
    h(2, d^)
