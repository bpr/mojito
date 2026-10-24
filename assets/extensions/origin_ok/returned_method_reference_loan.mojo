# A reference returned from a struct method, whose declared origin is a struct
# origin parameter, keeps its ultimate source alive when bound to a `ref` local.
# The struct origin parameter resolves to the origin the receiver's `ref[Self.o]` field
# borrows, so the loan roots at the owner (`xs`/`ys`) and it is not dropped while
# the reference is still live. Neither the view nor the returned reference
# borrows the `ref` binding it was built from exclusively, so the binding is
# read beside them, through a fieldwise or a written `ref[Self.o]`
# constructor alike.
@fieldwise_init
struct View[o: Origin[mut=False]]:
    var src: ref[o] List[Int]

    def at(self, i: Int) -> ref[Self.o] Int:
        return self.src[i]


struct WrittenView[o: Origin[mut=False]]:
    var src: ref[o] List[Int]

    def __init__(out self, ref[Self.o] src: List[Int]):
        self.src = src

    def at(self, i: Int) -> ref[Self.o] Int:
        return self.src[i]


@fieldwise_init
struct Cursor[o: Origin[mut=False]]:
    var src: ref[o] List[Int]
    var index: Int

    # A reference-yielding accessor that advances (a `mut self` `__next__` shape).
    def take(mut self) -> ref[Self.o] Int:
        var i = self.index
        self.index += 1
        return self.src[i]


@fieldwise_init
struct MutView[o: Origin[mut=True]]:
    var src: ref[o] List[Int]

    def at(self, i: Int) -> ref[Self.o] Int:
        return self.src[i]


def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    xs.append(30)
    ref rx = xs

    var v = View(rx)
    ref a = v.at(0)   # loan on xs keeps it alive past the binding's use
    print(a)          # 10
    print(len(rx))    # 3

    var wv = WrittenView(rx)
    ref d = wv.at(2)
    print(d)          # 30
    print(len(rx))    # 3

    var cursor = Cursor(rx, 1)
    ref b = cursor.take()
    print(b)          # 20
    ref c = cursor.take()
    print(c)          # 30

    var ys = List[Int]()
    ys.append(1)
    ys.append(2)
    ref ry = ys
    var mv = MutView(ry)
    ref w = mv.at(1)
    w = 99            # write through the returned handle
    print(ys[0], ys[1])   # 1 99
