# A method keyed on a type pack of its own is served by its template, on a
# plain struct and on a generic struct's instance: a `comptime for` and a
# `comptime if` over the pack, a static method, a method spreading its pack
# into another, and a `def` spreading a read or owned pack into one.
struct Box[T: Writable & Copyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def show[*Ts: Writable](self, *a: *Ts):
        print(self.v, Ts.length)
        comptime for i in range(Ts.length):
            print(a[i])

    def pick[*Ts: Writable](self, *a: *Ts) -> Int:
        comptime if Ts.length > 1:
            return 2
        else:
            return 1

    @staticmethod
    def count[*Ts: Writable](*a: *Ts) -> Int:
        return a.__len__()


struct Sink:
    def __init__(out self):
        pass

    def take[*Ts: Writable](self, *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i])

    def tagged[*Ts: Writable](self, tag: Int, *a: *Ts):
        print(tag, a.__len__())

    def drain[*Ts: Writable & Movable](self, var *a: *Ts):
        print("drain", a.__len__())

    def relay[*Ts: Writable](self, *a: *Ts):
        self.take(*a)
        self.tagged(3, *a)


def outer[*Us: Writable](*a: *Us):
    Sink().take(*a)
    Sink().tagged(7, *a)


def owned[*Us: Writable & Movable](var *a: *Us):
    Sink().drain(*a^)


def main():
    var b = Box[Int](5)
    b.show(1, "x", 2.5)
    print(b.pick(1), b.pick(1, 2))
    print(Box[Int].count(1, 2, 3))
    outer(1, "two")
    owned(1, "s")
    Sink().relay("r", 4)
