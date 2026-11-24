# A type member read through a struct roots at a name that resolves to no
# binding, whatever its case: a lowercase struct's type member constructs
# and applies as a capitalized one's does.


@fieldwise_init
struct Unit(ImplicitlyCopyable):
    pass


struct holder:
    comptime Elem = Int


struct pair[T: Copyable]:
    comptime Elem = Self.T
    comptime Of[U: Copyable] = List[U]


struct box[n: Int]:
    comptime W = SIMD[DType.int32, Self.n]


struct maker:
    comptime Of[n: Int] = Unit


struct m:
    comptime X = Int


struct S[T: AnyType]:
    comptime Get[U: AnyType] = List[U]
    comptime B = Self.Get[m.X]


def main():
    var a = holder.Elem(3)
    var b = pair[Int].Elem(4)
    var c = pair[Int].Of[Int]()
    c.append(5)
    var w = box[2].W(7)
    comptime u = maker.Of[3]()
    var d = S[Int].B()
    d.append(2)
    print(a, b, len(c), w, len(d))
