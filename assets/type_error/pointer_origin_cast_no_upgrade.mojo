# expect: cannot write through a Pointer with an immutable origin
# `unsafe_origin_cast` rebinds provenance but never upgrades a statically
# immutable capability: the cast itself is accepted, a write through its
# result is not.
def f(p: Pointer[Int, ImmUntrackedOrigin]):
    var q = p.unsafe_origin_cast[MutUntrackedOrigin]()
    q[] = 1

def main():
    pass
