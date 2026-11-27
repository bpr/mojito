# A struct instance over `origin_of` of an argument holds that argument's
# mutability in the `mut=` slot the origin governs: `True` over a `mut`,
# owned, or mutable `ref` argument, `False` over a read one, in the
# signature's result and at each call, so a method reading the slot (or a
# callee inferring its `mut` binder from a pointer field) sees it.
struct View[view_mut: Bool, //, view_origin: Origin[mut=view_mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def show(self):
        print(Self.view_mut, self.n)


def viewl(mut xs: List[Int]) -> View[origin_of(xs)]:
    return View[origin_of(xs)](len(xs))


def viewr(xs: List[Int]) -> View[origin_of(xs)]:
    return View[origin_of(xs)](len(xs))


def viewref(ref xs: List[Int]) -> View[origin_of(xs)]:
    return View[origin_of(xs)](len(xs))


def read_only(xs: List[Int]):
    viewref(xs).show()


struct Cursor[cursor_mut: Bool, //, cursor_origin: Origin[mut=cursor_mut]]:
    var src: Pointer[List[Int], Self.cursor_origin]

    def __init__(out self, ref[Self.cursor_origin] xs: List[Int]):
        self.src = Pointer(to=xs)

    def first(self) -> Int:
        return get(self.src[])


def get[m: Bool, //, o: Origin[mut=m]](ref[o] xs: List[Int]) -> Int:
    print(m)
    return xs[0]


struct Box:
    var items: List[Int]

    def __init__(out self):
        self.items = [4, 5, 6]

    def cursor_mut(mut self) -> Cursor[origin_of(self.items)]:
        ref source = self.items
        return Cursor(source)

    def cursor_ref(ref self) -> Cursor[origin_of(self.items)]:
        ref source = self.items
        return Cursor(source)


def main():
    var xs: List[Int] = [1, 2]
    viewl(xs).show()
    viewr(xs).show()
    viewref(xs).show()
    read_only(xs)
    var it: List[Int].IteratorType[origin_of(xs)] = xs.__iter__()
    for x in it:
        print(x)
    var b = Box()
    print(b.cursor_mut().first())
    print(b.cursor_ref().first())
