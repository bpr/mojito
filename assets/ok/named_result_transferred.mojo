# A named `out` result is transferred to the caller: its destructor runs once,
# where the caller's binding dies, and not when the callee returns. The same
# holds for the consuming `Tuple` members, which build their named result
# from elements moved out of the receiver.
struct Loud(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("deinit", self.id)


struct Holder(Movable):
    var a: Loud
    var b: Loud

    def __init__(out self, first: Int):
        self.a = Loud(first)
        self.b = Loud(first + 1)

    def renew(deinit self, out result: Holder):
        result = Holder(10)


def make(out result: Loud):
    result = Loud(5)


def main():
    var made = make()
    print("made", made.id)
    var old = Holder(1)
    var new = old^.renew()
    print("renewed", new.a.id)
    var t = (Loud(20), Loud(21))
    var r = t^.reverse()
    print("reversed", r[0].id, r[1].id)
    var u = (Loud(30), Loud(31))
    var c = u^.concat((Loud(32),))
    print("joined", len(c), c[2].id)
