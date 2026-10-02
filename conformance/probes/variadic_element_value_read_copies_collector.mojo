# PROBE (divergence): a value read of a homogeneous collector's element
# copies the whole collector.
#
# `var x = a[1]` over `*a: IC` copies every element of `a` and keeps one, so
# the other copies are never destroyed. The pinned Mojo copies the element
# alone. A `Tuple` local's element is copied alone on both sides.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   h / copy 2 / I2 / del 2 / h end / del 2 / del 1 / end
#   mojito: h / copy 1 / copy 2 / I2 / del 2 / h end / del 2 / del 1 / end
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


def h(*a: IC):
    print("h")
    var x = a[1]
    print(x)
    print("h end")


def main():
    var p = IC(1)
    var q = IC(2)
    h(p, q)
    print("end")
