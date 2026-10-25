# Converting a `List` to a `Span` — at an annotated binding, a `Span`
# parameter, or from a field — builds a view over the list where it lies:
# no element is copied and no temporary list is destroyed. An element read
# through the view is lent, as a `List` element is.
struct Dup(Copyable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("D", self.n)


struct Holder:
    var items: List[Dup]

    def __init__(out self):
        self.items = List[Dup]()
        self.items.append(Dup(7))

    def first(self) -> Int:
        var s: Span[Dup, origin_of(self.items)] = self.items
        return s[0].n


def g(d: Dup):
    print("g", d.n)


def total(s: Span[Dup, _]) -> Int:
    var t = 0
    for d in s:
        t += d.n
    return t


def main():
    var xs = List[Dup]()
    xs.append(Dup(1))
    xs.append(Dup(2))
    var s: Span[Dup, origin_of(xs)] = xs
    print(s[0])
    g(s[1])
    print(total(xs))
    var h = Holder()
    print(h.first(), total(h.items))
    xs.append(Dup(3))
    print(len(xs))
    print("end")
