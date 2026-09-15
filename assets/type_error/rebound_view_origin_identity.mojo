# expect: cannot implicitly convert 'P[origin_of(ys)]' value to 'P[origin_of(xs)]'
# The origin argument is part of a struct's identity: a view local bound over
# one origin cannot be rebound to a view over another, as at the pin.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var ys = List[Int]()
    ys.append(1)
    var p = P(Pointer(to=xs))
    p = P(Pointer(to=ys))
    p.src[][0] = 9
    print(ys[0])
