# A `ref` binding and an `as` binding keep the subject's own capability: over
# a mutable place they write through, over a temporary they read it; a guard
# sees the case's bindings and a false guard reaches the next case.
struct Big(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def make() -> Big:
    return Big(3)


def main():
    var b = Big(1)
    match b:
        case ref r:
            r.v = 2
    print("ref", b.v)
    match b:
        case _ as whole:
            whole.v = 4
    print("as", b.v)
    match make():
        case ref t:
            print("temp ref", t.v)
    match b:
        case x if x.v == 4:
            print("guard true", x.v)
        case _:
            print("guard false")
    match b:
        case x if x.v == 5:
            print("guard true", x.v)
        case _:
            print("guard false")
    print("end")
