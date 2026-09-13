# A callable struct that transfers its argument onward: the box moves through
# `__call__` into the collection, and the caller's binding is dead after the
# transfer. A box carrying a *borrowed* origin cannot make this trip upstream
# — instantiating the callable at that origin aliases the argument — so that
# shape is the `origin-carrying-callable-struct` conformance case.
@fieldwise_init
struct Box:
    var id: Int

@fieldwise_init
struct Stasher(def(mut List[Box], var Box)):
    var tag: Int

    def __call__(self, mut sink: List[Box], var box: Box):
        sink.append(box^)
        sink.append(Box(self.tag))

def main():
    var s = Stasher(5)
    var sink = List[Box]()
    var box = Box(9)
    s(sink, box^)
    print(sink[0].id, sink[1].id)
