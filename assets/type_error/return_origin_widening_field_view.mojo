# expect: cannot implicitly convert 'View[origin_of(self.items)]' value to 'View[origin_of(self)]'
# A view over one of the receiver's fields carries that field's origin: it
# does not widen to a declared `origin_of(self)` return, as at the pin. The
# exact `-> View[origin_of(self.items)]` spelling is accepted.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] src: List[Int], index: Int):
        self.src = Pointer(to=src)
        self.index = index

struct Box:
    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(7)

    def view(ref self) -> View[origin_of(self)]:
        ref source = self.items
        return View(source, 0)

def main():
    var b = Box()
    var v = b.view()
    print(v.src[][0])
