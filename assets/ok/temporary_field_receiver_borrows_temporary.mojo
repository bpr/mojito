# A field of an owned temporary lent to a borrow (a `ref self` getter's
# receiver, a view method's receiver, an argument the result borrows) is a
# projection of the materialized temporary, which lives as long as the
# borrower; a copied field of a returned reference reads that reference.
@fieldwise_init
struct V(Copyable, Movable):
    var xs: List[Int]
    var name: String

    def __add__(self, other: Self) -> Self:
        var out = List[Int]()
        for i in range(len(self.xs)):
            out.append(self.xs[i] + other.xs[i])
        return V(out^, self.name + other.name)

@fieldwise_init
struct W(Copyable, Movable):
    var v: V
    var tag: String

    def get(ref self) -> ref[self.v] V:
        return self.v

def mk() -> V:
    return V([7, 8], "héllo")

def items(v: V) -> ref[v.xs] List[Int]:
    return v.xs

def show(s: StringSpan[_]):
    print(s)

def main():
    print(V([1], "a").xs[0])
    var a = V([1], "a")
    var b = V([2], "b")
    print((a + b).xs[0])
    print(W(mk(), "t").v.xs[0])
    print(len(mk().xs), mk().xs[1])
    print(items(W(mk(), "t").v)[1])
    for c in mk().name.codepoints():
        print(c)
    show(mk().name)
    var w = W(mk(), "tag")
    var s = w.get().name
    print(s, w.tag)
