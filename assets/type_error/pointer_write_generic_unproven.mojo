# A generic pointee proves no `IsTriviallyDeinitable` fact, so the body
# cannot call `write`.
# expect: lacking evidence to prove correctness
def g[T: Movable](p: MutPointer[T, _], var v: T):
    p.write(v^)


def main():
    var x = 1
    g(Pointer(to=x), 4)
    print(x)
