# A symbolic-origin pointer field bound from a mutable local writes through,
# as upstream's per-instantiation judgment allows.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var p = P(Pointer(to=xs))
    p.src[][0] = 9
    print(p.src[][0])
