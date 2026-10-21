# Pinned against Mojo 1.2.0.dev2026101705 (2026-10-21): inside a `deinit
# self` body, a field only a `try` handler reads dies on the normal path
# right after the body's last raising call, before the non-raising calls
# after it, since only a raising call can reach the handler. A field read
# after the `try` lives to that read.
# Expected:
#   del b / after risky / end a / del a
#   risky raised / handler b / del b / end a / del a / done


struct Noisy(Movable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

    def __deinit__(deinit self):
        print("del", self.name)


def risky(flag: Bool) raises:
    if flag:
        raise Error("boom")


struct Holder(Movable):
    var a: Noisy
    var b: Noisy
    var flag: Bool

    def __init__(out self, flag: Bool):
        self.a = Noisy("a")
        self.b = Noisy("b")
        self.flag = flag

    def __deinit__(deinit self):
        try:
            risky(self.flag)
            print("after risky")
        except e:
            print("risky raised")
            print("handler", self.b.name)
        print("end", self.a.name)


def main():
    var h = Holder(False)
    _ = h^
    var g = Holder(True)
    _ = g^
    print("done")
