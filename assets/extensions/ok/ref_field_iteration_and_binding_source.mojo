# A `ref` field's stored handle is loaded out of its slot wherever it is
# taken whole: a `ref` binding of the field (read and written through), a
# loop iterating the field, a comprehension over it, and the field or such
# a binding passed at a `ref` parameter, each also through a `ref`
# binding of the holder.
struct Bag:
    var items: List[Int]

    def __init__(out self, var items: List[Int]):
        self.items = items^


@fieldwise_init
struct Holder[o: MutOrigin]:
    var bag: ref[Self.o] Bag

    def total(self) -> Int:
        ref s = self.bag
        var t = 0
        for x in s.items:
            t += x
        return t

    def bump(mut self):
        ref s = self.bag
        s.items.append(9)


@fieldwise_init
struct Walk[o: MutOrigin]:
    var src: ref[Self.o] List[Int]

    def total(self) -> Int:
        var t = 0
        for x in self.src:
            t += x
        return t

    def doubled(self) -> List[Int]:
        return [x * 2 for x in self.src]

    def grow(mut self):
        for ref x in self.src:
            x += 1

    def alias_arg(self) -> Int:
        ref s = self.src
        return takes_ref(s)

    def direct_ref_arg(self) -> Int:
        return takes_ref(self.src)


def takes_ref(ref l: List[Int]) -> Int:
    return len(l)


def main():
    var l = List[Int]()
    l.append(1)
    l.append(2)
    var b = Bag(l^)
    var h = Holder(b)
    h.bump()
    print(h.total())
    print(len(h.bag.items))

    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    var w = Walk(xs)
    print(w.total())
    w.grow()
    var d = w.doubled()
    print(d[0], d[1])
    print(w.alias_arg(), w.direct_ref_arg())
    ref r = w
    var u = 0
    for x in r.src:
        u += x
    ref s = r.src
    print(u, len(s), takes_ref(r.src), len([x * 2 for x in r.src]))
    print(xs[0] + xs[1])
