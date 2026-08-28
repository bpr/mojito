# expect: access to 'b.items' conflicts with live reference 'v'
# A ref-field view returned from an ordinary method lends the receiver:
# mutating the source while the view is live is rejected.
@fieldwise_init
struct View[
    view_mut: Bool, //,
    view_origin: Origin[mut=view_mut],
]:
    var src: ref[view_origin] List[Int]
    var index: Int

    def first(self) -> Int:
        return self.src[0]

struct Box:
    comptime ViewType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = View

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(1)
        self.items.append(2)

    def view(ref self) -> Self.ViewType[origin_of(self)]:
        ref source = self.items
        return View(source, 0)

def main():
    var b = Box()
    var v = b.view()
    b.items.append(99)
    print(v.first())
