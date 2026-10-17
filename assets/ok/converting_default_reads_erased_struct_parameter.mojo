# A converting default over a struct whose constructor reads its parameter,
# at a generic callee that removed the binder the parameter's type spells:
# each call leaving the argument out builds `Tag[k](2)` itself, so the
# callee's instance names no construction of the erased `Tag`.
struct Tag[n: Int](ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, v: Int):
        self.v = v * Self.n


def f(t: Tag[3] = 5):
    print(t.v)


def h[k: Int](t: Tag[k] = 2):
    print(t.v)


def main():
    f()
    f(6)
    h[4]()
    h[5](1)
