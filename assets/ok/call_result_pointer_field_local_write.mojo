# A local bound from a view-returning call keeps the call's origin bindings,
# so a write through its pointer field is judged like one through a directly
# constructed view.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def make(mut xs: List[Int]) -> P[origin_of(xs)]:
    return P(Pointer(to=xs))

def main():
    var xs = List[Int]()
    xs.append(7)
    var p = make(xs)
    p.src[][0] = 9
    print(xs[0])
