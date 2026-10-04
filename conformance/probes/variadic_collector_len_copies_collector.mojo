# PROBE (divergence): `len` of a homogeneous collector copies the whole
# collector.
#
# `len(a)` over `*a: IC` copies every element of `a` to take the length, and
# the copies are never destroyed. The pinned Mojo copies nothing.
#
# Observed 2026-10-03 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   del 2 / del 1 / 2 / end
#   mojito: copy 1 / copy 2 / del 2 / del 1 / 2 / end
#
# When fixed: promote to `assets/ok`.
struct IC(ImplicitlyCopyable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("I", self.n)

def count(*a: IC) -> Int:
    return len(a)


def main():
    var p = IC(1)
    var q = IC(2)
    print(count(p, q))
    print("end")
