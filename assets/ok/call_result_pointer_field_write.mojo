# A write through the pointer field of a view a call returns: the callee's
# return contract `P[origin_of(xs)]` binds the view's origin slot to the
# argument place, and its `mut xs` parameter makes that binding writable.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def make(mut xs: List[Int]) -> P[origin_of(xs)]:
    return P(Pointer(to=xs))

def main():
    var xs = List[Int]()
    xs.append(7)
    make(xs).src[][0] = 9
    print(xs[0])
