# PROBE (divergence): an owned pack's elements are destroyed after the first
# element's last use.
#
# The pinned Mojo prints both elements and then destroys them in reverse
# order. Mojito destroys both after printing the first, then prints the
# second: a silent wrong order. A top-level `def` and a method collector
# behave alike.
#
# Observed 2026-09-26 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   N1 / N2 / del 2 / del 1
#   mojito: N1 / del 1 / del 2 / N2
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


def take[*Ts: Writable & Movable](var *a: *Ts):
    comptime for i in range(a.__len__()):
        print(a[i])


def main():
    take(Noisy(1), Noisy(2))
