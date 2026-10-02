# PROBE (divergence): reading an element of a read type pack copies it.
#
# The pinned Mojo prints the element in place. Mojito's VM runs the element's
# copy constructor once for `print(a[0])` and never destroys the copy; the
# native backend runs it twice.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:            in / D1 / out / del 1 / end
#   mojito (VM):     in / copy 1 / D1 / out / del 1 / end
#   mojito (native): in / copy 1 / copy 1 / D1 / out / del 1 / end
#
# When fixed: promote to `assets/ok`.
struct Dup(Copyable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("D", self.n)


def show[*Ts: Writable](*a: *Ts):
    print("in")
    print(a[0])
    print("out")


def main():
    var d = Dup(1)
    show(d, 2)
    print("end")
