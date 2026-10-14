# A `comptime for` over a value pack or a reflected field-name list steps its
# `__iter__()` iterator through the `paramfor_*` stubs, as upstream does.
def g[n: Int]():
    print("g", n)


def ints[*vals: Int]():
    comptime for v in vals:
        g[v]()
        comptime if v > 1:
            print("big", v)


def nested[*vals: Int]():
    comptime for a in vals:
        comptime for b in vals:
            print(a * 10 + b)


def flags[*fs: Bool]():
    comptime for f in fs:
        print(f)


def floats[*xs: Float64]():
    comptime for x in xs:
        print(x)


def early[*vals: Int]():
    comptime for v in vals:
        comptime if v == 2:
            break
        print("e", v)


struct S[*values: Int]:
    @staticmethod
    def show():
        comptime for v in Self.values:
            print("s", v)


@fieldwise_init
struct P:
    var x: Int
    var yy: String


@fieldwise_init
struct E:
    pass


def names[T: AnyType]():
    comptime for n in reflect[T].field_names():
        print(n)
        comptime if n == "x":
            print("found x")


def main():
    ints[1, 2, 3]()
    nested[1, 2]()
    flags[True, False]()
    floats[1.5, 2.5]()
    early[1, 2, 3]()
    S[4, 5].show()
    names[P]()
    names[E]()
    comptime ns = reflect[P].field_names()
    comptime for n in ns:
        print("b", n)
