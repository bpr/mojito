# A type's compile-time member whose value is a construction, read through
# a type parameter (`T.V`) or the type itself (`A.V`): each read constructs
# one value and destroys it, and a field read off one (`T.V.a`, `T.W.p.a`)
# folds to the field alone, constructing nothing. A parameterized member
# (`T.get[3]`) is its body at the bound arguments, read the same way.
struct P(ImplicitlyCopyable, Movable, Writable):
    var a: Int

    def __init__(out self, a: Int):
        self.a = a

    def __deinit__(deinit self):
        print("del", self.a)

    def write_to(self, mut w: Some[Writer]):
        w.write("P(", self.a, ")")


@fieldwise_init
struct Q(ImplicitlyCopyable, Movable):
    var p: P
    var b: Int


trait HasP:
    comptime V: P
    comptime W: Q
    comptime get[i: Int]: P


struct A(HasP):
    comptime V = P(2)
    comptime W = Q(P(4), 5)
    comptime get[i: Int] = P(i + 1)


struct B[n: Int](HasP):
    comptime V = P(Self.n + 1)
    comptime W = Q(P(Self.n), Self.n * 2)
    comptime get[i: Int] = P(i * Self.n)


comptime G = P(9)


def look(p: P):
    print("look", p.a)


def show[T: HasP]():
    print(T.V.a)
    print(T.V)
    look(T.V)
    var v = T.V
    print("v", v.a)
    print(T.W.b, T.W.p.a)
    print(T.W.p)
    print(T.get[3])
    print(T.get[3].a)


def main():
    print(A.V.a, A.W.p.a, G.a)
    show[A]()
    show[B[6]]()
