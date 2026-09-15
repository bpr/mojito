# expect: write through a Pointer whose origin is immutable
# A read `self` receiver binds the returned view's slot immutably, so a
# write through the view's pointer field rejects even on a `var` holder.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

struct Holder:
    var xs: List[Int]

    def __init__(out self):
        self.xs = List[Int]()
        self.xs.append(7)

    def view(self) -> P[origin_of(self.xs)]:
        return P(Pointer(to=self.xs))

def main():
    var h = Holder()
    h.view().src[][0] = 9
    print(h.xs[0])
