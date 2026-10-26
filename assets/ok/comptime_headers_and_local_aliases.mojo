# `comptime` headers and local bindings: a static call on `Self` in a
# `comptime for` header, a module list declared after the `def` that
# iterates it, a local type alias constructing in an unrolled body, an
# unread local application returning a reference-holding iterator, and a
# nested `def` keyed on a type pack.
struct V[*Ts: Writable]:
    def __init__(out self):
        pass

    @staticmethod
    def __len__() -> Int:
        return len(Self.Ts)

    def show(self):
        comptime for i in range(Self.__len__()):
            print(i)


def later():
    comptime for x in L:
        print(x)


comptime L = [10, 20]


def outer():
    def inner[*Ts: Writable](*args: *Ts):
        comptime for i in range(len(Ts)):
            print(args[i])

    inner(1, "a")


def main():
    V[Int, String, Bool]().show()
    later()
    comptime for i in range(2):
        comptime A = Int
        print(A(3) + i)
    comptime d = {"a": 1}
    comptime ks = d.keys()
    outer()
