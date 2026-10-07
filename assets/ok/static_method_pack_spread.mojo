# A pack spread into a static method's collector, alone or after leading
# arguments, and an explicit `[*Ts]` forwarding the pack into a static's
# result type, all served by the spreading `def`'s template.


struct B[*Ts: Writable](Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


struct U:
    @staticmethod
    def show[*Ts: Writable](*args: *Ts):
        comptime for i in range(Ts.length):
            print(args[i], end=";")
        print()

    @staticmethod
    def inner[*Ts: Writable](mut w: String, n: Int, *args: *Ts):
        w.write(n, ":")
        comptime for i in range(Ts.length):
            args[i].write_to(w)

    @staticmethod
    def mk[*Ts: Writable](n: Int) -> B[*Ts]:
        return B[*Ts](n + Ts.length)


def fwd[*Ts: Writable](*a: *Ts):
    U.show(*a)


def outer[*Ts: Writable](*args: *Ts) -> String:
    var b = String()
    U.inner(b, U.mk[*Ts](1).n, *args)
    return b^


def main():
    U.show(1, "a")
    fwd(3, "b", True)
    print(outer(3, "y", True))
    print(U.mk[Int, String](1).n)
