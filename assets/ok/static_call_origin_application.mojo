# A static method called through an origin-bearing type application
# (`View[origin_of(w)].make(w)`) partitions the origin slot out of the
# receiver application as construction does; a static binding `ref [Self.o]`
# arguments is a constructor in all but name — the explicit origin is checked
# against the argument and the result keeps it lent (`w` outlives the pointer
# reads). Bare statics retain their `ref`/`mut` argument places.

struct View[o: Origin[mut=True]]:
    var r: Pointer[Int, Self.o]

    def __init__(out self, ref [Self.o] x: Int):
        self.r = Pointer(to=x)

    @staticmethod
    def make(ref [Self.o] x: Int) -> View[Self.o]:
        return View[Self.o](x)

    @staticmethod
    def two() -> Int:
        return 2


struct Tally:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    @staticmethod
    def peek(ref x: Int) -> Int:
        return x + 1

    @staticmethod
    def bump(mut x: Int):
        x += 10


def main():
    var w = 4
    print(View[origin_of(w)].two())
    var v = View[origin_of(w)].make(w)
    v.r[] = 11
    print(v.r[], w)
    var z = 1
    var u = View.make(z)
    print(u.r[])
    var xs: List[Int] = [1, 2, 3]
    var s = Span[Int, origin_of(xs)](xs)
    print(len(s))
    var t = 4
    print(Tally.peek(t))
    Tally.bump(t)
    print(t)
