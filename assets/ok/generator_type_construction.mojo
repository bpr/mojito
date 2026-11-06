# A call through a type generator constructs the type its application
# names: a local or module `comptime` bound to a type-bodied alias read
# unapplied, applied and called (`P[Int]()`), and a local alias of that
# application called (`Q(3, 4)`).

@fieldwise_init
struct Pt[T: Copyable & Deinitable & Writable](Copyable, Writable):
    var x: Self.T
    var y: Self.T


comptime MkList[T: Copyable] = List[T]
comptime MkPt[T: Copyable & Deinitable & Writable] = Pt[T]
comptime M = MkPt


def main():
    comptime P = MkList
    var z = P[Int]()
    z.append(3)
    var y: P[Int] = [1, 2]
    print(len(y), len(z), z[0])
    comptime L = P[String]
    var w = L()
    w.append("w")
    print(len(w), w[0])
    comptime N = MkPt
    var a = N[Int](1, 2)
    print(a.x, a.y)
    var b = M[String]("u", "v")
    print(b.x, b.y)
    comptime Q = N[Int]
    var c = Q(3, 4)
    print(c.x + c.y)
