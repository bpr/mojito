# expect: cannot implicitly convert 'View[origin_of(b.items)]' value to 'View[origin_of(b)]'
# A free function's returned view over one of its parameter's fields carries
# that field's origin: it does not widen to a declared `origin_of(b)` return,
# as at the pin.
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

def view(ref b: Box) -> View[origin_of(b)]:
    ref s = b.items
    return View(s, 0)

def main():
    var b = Box()
    var v = view(b)
    print(v.src[][0])
