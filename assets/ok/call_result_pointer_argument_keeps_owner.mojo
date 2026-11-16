# A pointer an ordinary call returns, passed on to another call, keeps
# the storage its origin names alive until that call reads it: an
# in-place `Pointer(to=)`, a pointer variable, a nested identity, and
# an owner whose destructor runs only after the read.
def id[o: ImmOrigin](p: Pointer[String, o]) -> Pointer[String, o]:
    return p


def read(p: Pointer[String, _]) -> String:
    return p[]


def idi[o: ImmOrigin](p: Pointer[Int, o]) -> Pointer[Int, o]:
    return p


def readi(p: Pointer[Int, _]) -> Int:
    return p[]


struct Noisy:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def idn[o: ImmOrigin](p: Pointer[Noisy, o]) -> Pointer[Noisy, o]:
    return p


def show(p: Pointer[Noisy, _]):
    print("show", p[].v)


def main():
    var i = 7
    print(readi(idi(Pointer(to=i))))
    var x = String("via var")
    var p = Pointer(to=x)
    print(read(id(p)))
    var y = String("nested")
    print(read(id(id(Pointer(to=y)))))
    var n = Noisy(5)
    show(idn(Pointer(to=n)))
    print("end")
