# expect: expression must be mutable for in-place operator destination
# An augmented store through the field's dereference takes the pointer's
# capability, not the holder binding's: `p` is a `var`, `xs` is not.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def show(xs: List[Int]):
    var p = P(Pointer(to=xs))
    p.src[][0] += 1
    print(p.src[][0])

def main():
    var xs = List[Int]()
    xs.append(7)
    show(xs)
