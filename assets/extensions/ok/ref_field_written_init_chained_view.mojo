# A `ref` field passed to a written constructor's `ref` parameter binds the
# parameter to the field's referent, not to the field's slot: a view built by
# `shifted()` over a temporary view chained straight off `b.pane()` still
# borrows `b.items` after the temporary dies. Read, `ref`-returning,
# span-building, and assigning methods chained on that view all reach the
# list. A `mut` argument or receiver bound to a `ref` field likewise writes
# the referent, even when the callee reassigns it whole.
struct Pane[m: Bool, //, o: Origin[mut=m]]:
    var items: ref[Self.o] List[Int]
    var start: Int

    @no_inline
    def __init__(out self, ref[Self.o] items: List[Int], start: Int):
        self.items = items
        self.start = start

    def shifted(self) -> Pane[Self.o]:
        return Pane(self.items, self.start + 1)

    def first(self) -> Int:
        return self.items[self.start]

    def at(self) -> ref[Self.o] Int:
        return self.items[self.start]

    def view(self) -> Span[Int, Self.o]:
        return Span(self.items)

    def __getitem__(self, i: Int) -> Int:
        return self.items[self.start + i]

@fieldwise_init
struct Counter(Copyable, Movable):
    var n: Int

    def reset(mut self):
        self = Counter(100)

struct Holder[o: MutOrigin]:
    var c: ref[Self.o] Counter

    @no_inline
    def __init__(out self, ref[Self.o] c: Counter):
        self.c = c

    def run(mut self):
        self.c.reset()

def bump(mut c: Counter):
    c = Counter(7)

struct Board:
    comptime PaneType[m: Bool, //, o: Origin[mut=m]] = Pane

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(5)
        self.items.append(6)
        self.items.append(7)

    def pane(ref self) -> Self.PaneType[origin_of(self)]:
        ref source = self.items
        return Pane(source, 0)

def main():
    var b = Board()
    print(b.pane().first())
    print(b.pane().shifted().first())
    print(b.pane().shifted()[1])
    ref r = b.pane().shifted().at()
    print(r)
    var s = b.pane().shifted().view()
    print(s[2])
    var p = b.pane()
    var q = p.shifted()
    print(q.first())
    b.pane().shifted().at() = 9
    print(b.items[1])
    var c = Counter(1)
    var h = Holder(c)
    bump(h.c)
    print(c.n)
    var d = Counter(2)
    var g = Holder(d)
    bump(g.c)
    g.run()
    print(d.n)
