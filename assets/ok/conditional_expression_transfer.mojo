# A conditional expression's result is an owned temporary: each arm
# initializes it (a `^` arm by its move initializer, a place arm by its copy
# initializer, a temporary arm as it stands), and its consumer takes it as
# any other rvalue, so it is never copied again. An owned parameter takes its
# storage, a binding or a `return` takes it in place, and a read parameter or
# receiver borrows it until the call returns.
struct M(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
    def __deinit__(deinit self):
        print("del", self.v)
    def show(self):
        print("show", self.v)
    def eat(var self):
        print("eat", self.v)

def take(var s: M):
    print("take", s.v)

def look(s: M):
    print("look", s.v)

def pick(var a: M, var b: M, c: Bool) -> M:
    return a^ if c else b^

def bind_ref(ref r: M, c: Bool):
    var t = r if c else M(3)
    print("t", t.v)

def main():
    var y = M(4)
    var z = M(5)
    take(y^ if z.v > 0 else z^)
    var a = M(1)
    var b = M(2)
    var r = a^ if b.v > 0 else b^
    print("r", r.v)
    var c = M(6)
    var d = M(7)
    look(c^ if d.v > 0 else d^)
    var e = pick(M(8), M(9), True)
    print("e", e.v)
    take(M(10) if r.v > 0 else M(11))
    var f = M(12)
    take(f.copy() if r.v > 0 else M(13))
    (M(20) if r.v > 0 else M(21)).eat()
    (M(22) if r.v < 0 else M(23)).show()
    var g = M(30)
    bind_ref(g, True)
    print("g", g.v)
    for i in range(2):
        var p = M(40 + i)
        var q = M(50 + i)
        take(p^ if i == 0 else q^)
