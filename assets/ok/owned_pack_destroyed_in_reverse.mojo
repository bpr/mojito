# An owned (`var`) pack lives until the last use of any element read out of
# it, and its elements are then destroyed last to first. A top-level `def`, a
# method, a homogeneous collector, and a pack nothing reads behave alike.
struct Noisy(Movable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("N", self.n)


struct Sink:
    def __init__(out self):
        pass

    def each[*Ts: Writable & Movable](self, label: Int, var *a: *Ts):
        comptime for i in range(a.__len__()):
            print(label, a[i])
        print("each end")


def take[*Ts: Writable & Movable](var *a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])
    print("take end")


def same(var *a: Noisy):
    print(a[0])
    print(a[1])
    print("same end")


def unused[*Ts: Writable & Movable](var *a: *Ts):
    print("unused end")


def mixed[*Ts: Writable & Movable](var *a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])
    print("mixed end")


def partial(var *a: Noisy):
    print(a[1])
    print("partial end")


def main():
    take(Noisy(1), Noisy(2))
    same(Noisy(3), Noisy(4))
    Sink().each(0, Noisy(5), Noisy(6), Noisy(7))
    unused(Noisy(8), Noisy(9))
    mixed(10, Noisy(11), "s", Noisy(12))
    partial(Noisy(13), Noisy(14), Noisy(15))
