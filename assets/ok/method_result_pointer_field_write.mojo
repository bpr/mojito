# A write through the pointer field of a view a `mut self` method returns:
# `origin_of(self.xs)` binds the view to the receiver, and the receiver's
# mutable capability makes the binding writable.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

struct Holder:
    var xs: List[Int]

    def __init__(out self):
        self.xs = List[Int]()
        self.xs.append(7)

    def view(mut self) -> P[origin_of(self.xs)]:
        return P(Pointer(to=self.xs))

def main():
    var h = Holder()
    h.view().src[][0] = 9
    print(h.xs[0])
