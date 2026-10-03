# A generic nested `def` with a by-value capture (`{var x}`) snapshots `x`
# when it is declared, so a later `x = 100` does not reach it: the pin and
# the VM both print 10. Natively it is refused: "generic retained callable
# `outer$inner` captures by value" — the per-call specialization passes a
# by-reference environment as leading arguments, but a copied capture lives
# only in the closure value the direct call no longer reads (roadmap 3.83).
# Promote to `assets/ok` once the native run prints 10.
def outer() -> Int:
    var x = 5

    def inner[k: Int]() {var x} -> Int:
        return x * k

    x = 100
    return inner[2]()


def main():
    print(outer())
