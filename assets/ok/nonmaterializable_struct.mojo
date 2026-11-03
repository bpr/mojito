# A register-passable struct declaring `@__nonmaterializable(Big)`
# materializes to `Big`, through `Big`'s implicit constructor, where its
# value becomes a run-time one: an inferred binding, a display element, and
# a deduced type parameter. A call on the value itself keeps the struct.


struct Big(ImplicitlyCopyable, Writable):
    var v: Int

    def __init__(out self, *, v: Int):
        self.v = v

    @implicit
    def __init__(out self, lit: Lit):
        self.v = lit.k * 10

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Big(", self.v, ")")


@fieldwise_init
@__nonmaterializable(Big)
struct Lit(TrivialRegisterPassable):
    var k: Int

    def half(self) -> Int:
        return self.k // 2


def show[T: Writable & Copyable](x: T):
    print(rebind[Big](x))


def main():
    var x = Lit(4)
    print(x.v)
    print(Lit(6).half())
    var t = (Lit(1), 2)
    print(t[0], t[1])
    var a = [Lit(2), Lit(3)]
    print(a[1].v)
    show(Lit(5))
