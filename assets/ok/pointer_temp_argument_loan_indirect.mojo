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
