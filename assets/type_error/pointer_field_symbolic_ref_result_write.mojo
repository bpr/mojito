# expect: write through a Pointer whose origin mutability is not known here
# A store through the reference a `__getitem__` returns, reached through a
# pointer field over the struct's `Origin[mut=m]` binder, rejects in the
# method body however the receiver was constructed.
struct Box(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __getitem__(ref self, i: Int) -> ref[self.v] Int:
        return self.v

struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Box, Self.o]

    def __init__(out self, ref[Self.o] xs: Box):
        self.src = Pointer(to=xs)

    def put(mut self, x: Int):
        self.src[][0] = x

def main():
    var b = Box(7)
    var v = View(b)
    v.put(9)
    print(b.v)
