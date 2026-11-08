# expect: write through a Pointer whose origin mutability is not known here
# A store through the reference a `__getitem__` returns, reached through a
# `Pointer[T, o]` parameter over the function's own `Origin[mut=m]` binder,
# rejects in the generic body, as a direct store through the pointer does.
struct Box(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __getitem__(ref self, i: Int) -> ref[self.v] Int:
        return self.v

def poke[m: Bool, //, o: Origin[mut=m]](p: Pointer[Box, o]):
    p[][0] = 9

def main():
    var b = Box(7)
    poke(Pointer(to=b))
    print(b.v)
