# expect: returned reference escapes storage outside its declared origin
# A method-returned ref-field view may not outlive its source: a body that
# borrows a local cannot satisfy the declared `origin_of(self)` result origin.
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

    def bad_view(ref self) -> Self.ViewType[origin_of(self)]:
        var local = List[Int]()
        local.append(42)
        ref source = local
        return View(source, 0)

def main():
    var b = Box()
    var v = b.bad_view()
    print(v.first())
