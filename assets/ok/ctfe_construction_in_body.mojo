# A body's compile-time construction of a struct (`P(3)`), a method chain
# on one or on a module constant holding one (`E.get()`), and a
# construction over a keyed application are requests the elaborator below
# MIR serves, as is a compile-time expression over a requested binding
# (`comptime(a + 1)`).
def k[n: Int]() -> Int:
    return n * 10

def f(x: Int) -> Int:
    return x * 2

@fieldwise_init
struct P(Copyable, Movable):
    var x: Int

    def get(self) -> Int:
        return self.x + 1

comptime E = P(7)

def main():
    comptime p = P(3)
    print(p.x)
    comptime q = P(3).get()
    print(q)
    comptime r = P(k[2]())
    print(r.x)
    comptime a = f(1)
    print(comptime(a + 1))
    comptime e = E.get()
    print(e, comptime(E.get() * 2))
