# Explicit origin arguments in direct struct applications: return types,
# local annotations, and constructor expressions. The argument is validated
# (an origin, with sufficient mutability) and erased from the struct identity.
# The source is stored through `Pointer[T, Self.o]` fields.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

    def first(self) -> Int:
        return self.src[][0]

struct TakeIter[o: Origin[mut=True]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def get(self) -> Int:
        return self.src[][0]

struct Box:
    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(11)

    def view(ref self) -> View[origin_of(self)]:
        ref source = self.items
        return View(source, 0)

def main():
    var b = Box()
    var v = b.view()
    print(v.first())
    var data = List[Int]()
    data.append(5)
    ref r = data
    var annotated: View[origin_of(data)] = View(r, 0)
    print(annotated.first())
    var taken = TakeIter[origin_of(data)](r)
    print(taken.get())
