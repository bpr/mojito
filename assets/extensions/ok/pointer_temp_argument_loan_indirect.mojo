# A callable value whose parameter type is a parametric origin-carrying
# struct. Mojito erases the origin and anchors the temporary's loan across
# the indirect call; the pin refuses a parametric function as an argument at
# all, and binding the origin concretely then mismatches the callee's own
# local.
# A loan-carrying temporary argument anchors across a callable-value call
# exactly as across a direct call: the hidden argument slot's loan keeps `n`
# alive until the callee bound to `f` has run.
@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Int, Self.o]

def read(h: Holder) -> Int:
    return h.src[]

def apply(f: def(h: Holder) -> Int, n: Int) -> Int:
    var local = n
    return f(Holder(Pointer(to=local)))

def main():
    print(apply(read, 7))
