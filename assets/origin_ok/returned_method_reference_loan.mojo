# A reference returned from a struct method, whose declared origin is a struct
# origin parameter, keeps its ultimate source alive when bound to a `ref` local.
# The struct origin parameter resolves to the origin the receiver's
# `Pointer[T, Self.o]` field borrows, so the loan roots at the owner (`xs`/`ys`)
# and it is not dropped while the reference is still live. Previously the
# returned reference recorded no loan on its source, which was dropped early and
# left the handle dangling. `View` and `Cursor` borrow `xs` from the owner
# directly rather than through one shared `ref rx = xs` binding: two pointer
# borrows taken through a single `ref` binding are still rejected.
struct View[o: Origin[mut=False]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def at(self, i: Int) -> ref[Self.o] Int:
        return self.src[][i]


struct Cursor[o: Origin[mut=False]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

    # A reference-yielding accessor that advances (a `mut self` `__next__` shape).
    def take(mut self) -> ref[Self.o] Int:
        var i = self.index
        self.index += 1
        return self.src[][i]


struct MutView[o: Origin[mut=True]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def at(self, i: Int) -> ref[Self.o] Int:
        return self.src[][i]


def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    xs.append(30)

    var v = View(xs)
    ref a = v.at(0)   # loan on xs keeps it alive past the binding's use
    print(a)          # 10

    var cursor = Cursor(xs, 1)
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
