# Reads through the pointer field of a view a call returns — a nominal
# subscript, a method call, and a field read — each keep the temporary alive
# in a hidden slot for the access.
@fieldwise_init
struct Box:
    var v: Int

@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

@fieldwise_init
struct Q[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Box, Self.o]

struct Holder:
    var xs: List[Int]

    def __init__(out self):
        self.xs = List[Int]()
        self.xs.append(7)

    def view(self) -> P[origin_of(self.xs)]:
        return P(Pointer(to=self.xs))

def make(mut xs: List[Int]) -> P[origin_of(xs)]:
    return P(Pointer(to=xs))

def boxed(mut b: Box) -> Q[origin_of(b)]:
    return Q(Pointer(to=b))

def main():
    var xs = List[Int]()
    xs.append(7)
    print(make(xs).src[][0])
    print(make(xs).src[].__len__())
    var h = Holder()
    print(h.view().src[][0])
    print(h.view().src[].__len__())
    var b = Box(5)
    print(boxed(b).src[].v)
    print(boxed(b).src[].v + 1)
