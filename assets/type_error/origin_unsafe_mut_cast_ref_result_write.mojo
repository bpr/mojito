# A `ref` result over `o.unsafe_mut_cast[False]()` is immutable whatever `o`
# is, so the call is no assignment target.
# expect: expression must be mutable in assignment
def g[o: MutOrigin](ref[o] x: Int) -> ref[o.unsafe_mut_cast[False]()] Int:
    return x


def main():
    var x = 1
    g(x) = 5
    print(x)
