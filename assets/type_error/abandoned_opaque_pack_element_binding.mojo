# An element of an unbound variadic pack is an archetype of the pack's bound:
# a binding of one that the bound does not prove `Deinitable` must be
# destroyed explicitly, and its message names the composition's first trait.
# expect: 'y' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Defaultable'
def f[*Ts: Movable & Defaultable & Writable]():
    var y = Ts[0]()
    print(y)


def main():
    f[Int]()
