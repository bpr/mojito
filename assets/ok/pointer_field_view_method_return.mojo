# An ordinary (non-protocol) method hands a pointer-field view struct across
# its return: the call lends the receiver to the result, so the source stays
# live for the view's whole life even without any later direct use.
struct View[
    view_mut: Bool, //,
    view_origin: Origin[mut=view_mut],
]:
    var src: Pointer[List[Int], Self.view_origin]
    var index: Int

    def __init__(out self, ref[Self.view_origin] xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

    def first(self) -> Int:
        return self.src[][0]

struct Box:
    comptime ViewType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = View[view_origin]

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(1)
        self.items.append(2)
        self.items.append(3)

    def view(ref self) -> Self.ViewType[origin_of(self)]:
        ref source = self.items
        return View(source, 0)

def main():
    var b = Box()
    var v = b.view()
    print(v.first())
