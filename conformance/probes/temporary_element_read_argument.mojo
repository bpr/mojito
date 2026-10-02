# PROBE (divergence): an element of a temporary container cannot be passed to
# a read parameter.
#
# `g(make()[0])` is rejected with "value of type 'Dup' cannot be implicitly
# copied ... (ordinary value read through a reference result)". The pinned
# Mojo lends the element and destroys the temporary after the call.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   g 40 / del 40 / end
#   mojito: rejected by the checker
#
# When fixed: promote to `assets/ok`.
struct Dup(Copyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)


def g(d: Dup):
    print("g", d.n)


def make() -> List[Dup]:
    var r = List[Dup]()
    r.append(Dup(40))
    return r^


def main():
    g(make()[0])
    print("end")
