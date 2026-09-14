# Bound from a `mut` parameter, the binder is mutable.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def show(mut xs: List[Int]):
    var p = P(Pointer(to=xs))
    p.src[][0] = 9
    print(p.src[][0])

def main():
    var xs = List[Int]()
    xs.append(7)
    show(xs)
    print(xs[0])
