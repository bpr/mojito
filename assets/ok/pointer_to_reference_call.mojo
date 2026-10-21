# `Pointer(to=)` a call that returns a reference points at the place the
# reference names: a field accessor, `Optional.unsafe_value`, and a method
# reading its own accessor through an origin-cast pointer.


struct W:
    var x: Int

    def __init__(out self):
        self.x = 4

    def get(ref self) -> ref[self.x] Int:
        return self.x

    def whole(ref self) -> ref[self] Int:
        return Pointer(to=self.get()).unsafe_origin_cast[origin_of(self)]()[]


def main():
    var w = W()
    var p = Pointer(to=w.get())
    print(p[])
    var o = Optional(3)
    var q = Pointer(to=o.unsafe_value())
    print(q[])
    print(p[] + q[])
    ref r = w.whole()
    r = 9
    print(w.x)
