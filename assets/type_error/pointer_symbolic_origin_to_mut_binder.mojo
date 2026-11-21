# expect: type mismatch for argument
# A callable's own origin binder whose mutability is itself a parameter
# (`o: Origin[mut=m]`) does not convert to a `MutOrigin` binder: the body
# cannot prove the mutable capability, so the pin rejects the forward ("value
# passed to 'p' cannot be converted from 'Pointer[Int, o]' to 'Pointer[Int,
# o]'"), as it rejects an immutable origin.
def w[o: MutOrigin](p: Pointer[Int, o]):
    p[] = 3

def g[m: Bool, o: Origin[mut=m], //](p: Pointer[Int, o]):
    w(p)

def main():
    var x = 1
    g(Pointer(to=x))
    print(x)
