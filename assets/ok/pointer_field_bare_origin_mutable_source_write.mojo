# A bare `o: Origin` binder bound from a mutable local writes through.
@fieldwise_init
struct P[o: Origin]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var p = P(Pointer(to=xs))
    p.src[][0] = 9
    print(xs[0])
