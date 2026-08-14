# expect: cannot upgrade capability
# `origin_cast` rebinds provenance but never upgrades a statically immutable
# capability to a mutable one.
def f(p: Pointer[Int, ImmutUntrackedOrigin]) -> Int:
    var q = p.origin_cast[MutUntrackedOrigin]()
    return q[]

def main():
    pass
