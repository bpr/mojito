# expect: write through a Pointer whose origin is immutable
# The callee holds `xs` read-only, so the view it returns over
# `origin_of(xs)` binds the slot immutably whatever the caller's `var` allows.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def make(xs: List[Int]) -> P[origin_of(xs)]:
    return P(Pointer(to=xs))

def main():
    var xs = List[Int]()
    xs.append(7)
    make(xs).src[][0] = 9
    print(xs[0])
