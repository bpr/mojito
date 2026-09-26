# A leading-dot static call whose struct parameters only the expected type
# supplies, `.start(n)` against `Counter[Self.T]`, fails on Mojito with
# "cannot infer type parameter 'T' of 'Counter' from the arguments". The
# pin prints "20 30". Spelled `Counter[Self.T].start(n)` it runs on both.
@fieldwise_init
struct Counter[T: Copyable & Deinitable](Copyable, Movable):
    var n: Int

    @staticmethod
    def start(n: Int) -> Counter[Self.T]:
        return Counter[Self.T](n * 10)


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def counter(self, n: Int) -> Counter[Self.T]:
        return .start(n)


def main():
    print(Shelf[Int](1).counter(2).n, Shelf[String]("x").counter(3).n)
