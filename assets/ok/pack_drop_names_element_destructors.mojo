# A drop of a pack aggregate names each element's destructor, as upstream's
# `Tuple.__del__` and `VariadicPack.__deinit__` do: a tuple's storage, a
# nested tuple's included, destroys its elements first to last, a
# heterogeneous owning collector last to first, and an element whose
# destruction runs no code is skipped.


@fieldwise_init
struct Noisy(Movable):
    var n: Int

    def __deinit__(deinit self):
        print("del", self.n)


@fieldwise_init
struct Holder(Movable):
    var pair: Tuple[Noisy, String]
    var k: Int


def make() -> Tuple[Noisy, String, Noisy]:
    return (Noisy(1), String("hi"), Noisy(2))


def collect[*Ts: Movable](var *args: *Ts):
    print("collected", len(args))


def main():
    var t = make()
    print(t[1])
    var nested = (Noisy(3), (Noisy(4), 5, String("x")), Noisy(6))
    print(nested[1][2])
    var h = Holder((Noisy(7), String("y")), 8)
    print(h.k)
    collect(Noisy(9), String("s"), 10, Noisy(11))
    print("end")
