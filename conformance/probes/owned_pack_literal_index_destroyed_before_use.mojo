# PROBE (divergence): an owned heterogeneous pack whose last use is a
# literal-index element read is destroyed before that read is consumed.
#
# The pinned Mojo destroys the whole pack and then prints the element it
# read out of it. Mojito keeps the pack alive until the `print` returns.
# A `comptime for` over the same pack, and a homogeneous `var *a: Noisy`
# collector, print first on both.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   del 2 / del 1 / N1 / het_one end
#   mojito: N1 / del 2 / del 1 / het_one end
#
# When decided: match the pin, or record the difference in
# `docs/non-goals.md` if the pin's order reads a destroyed value.
struct Noisy(Movable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("N", self.n)


def het_one[*Ts: Writable & Movable](var *a: *Ts):
    print(a[0])
    print("het_one end")


def main():
    het_one(Noisy(1), Noisy(2))
