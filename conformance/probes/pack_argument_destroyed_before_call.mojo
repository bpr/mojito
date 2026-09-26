# PROBE (divergence): a local passed into a read type pack is destroyed
# before the call runs.
#
# The pinned Mojo prints the element inside the call and destroys `y` after
# it returns. Mojito runs `y.__del__` before the call's body and then prints
# the element anyway: a silent wrong order. A top-level `def` and a method
# collector behave alike.
#
# Observed 2026-09-26 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   in / N6 / out / del 6 / end
#   mojito: del 6 / in / N6 / out / end
#
# When fixed: promote to `assets/ok`.
struct Noisy(Movable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __del__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("N", self.n)


def show[*Ts: Writable](*a: *Ts):
    print("in")
    print(a[0])
    print("out")


def main():
    var y = Noisy(6)
    show(y, 5)
    print("end")
