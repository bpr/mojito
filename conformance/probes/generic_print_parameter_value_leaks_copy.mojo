# PROBE (divergence): `print` of a type-parameter-typed value in a generic
# `def` leaks a copy.
#
# `print(y)` with `y: T` copies `y` and never destroys the copy, and the
# `x.copy()` temporary passed to `print` is never destroyed either. The pinned
# Mojo reads `y` in place and destroys the temporary after the call.
#
# Observed 2026-10-03 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   copy 1 / I1 / del 1 / copy 1 / I1 / del 1 / f end / del 1 / end
#   mojito: copy 1 / copy 1 / del 1 / I1 / copy 1 / I1 / f end / del 1 / end
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

def f[T: ImplicitlyCopyable & Writable & Deinitable](x: T):
    var y = x
    print(y)
    print(x.copy())
    print("f end")


def main():
    var p = IC(1)
    f(p)
    print("end")
