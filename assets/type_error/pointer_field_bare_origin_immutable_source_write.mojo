# expect: write through a Pointer whose origin is immutable
# A bare `o: Origin` binder is judged per binding too, as the pin does.
@fieldwise_init
struct P[o: Origin]:
    var src: Pointer[List[Int], Self.o]

def show(xs: List[Int]):
    var p = P(Pointer(to=xs))
    p.src[][0] = 9

def main():
    var xs = List[Int]()
    xs.append(7)
    show(xs)
