# expect: cannot transfer a non-implicitly-copyable indexed value
# An element of an owned pack collector cannot be transferred out by
# subscript: `a[i]^` is rejected, as the pinned Mojo rejects it ("expression
# does not designate a value with an origin"). The `def` is served by its
# template, whose collector is a `VariadicPack`.
struct Noisy(Movable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def write_to(self, mut writer: Some[Writer]):
        writer.write("N", self.n)


def consume[T: Writable & Movable](var x: T):
    print("consume", x)


def take[*Ts: Writable & Movable](var *a: *Ts):
    comptime for i in range(a.__len__()):
        consume(a[i]^)


def main():
    take(Noisy(1), Noisy(2))
