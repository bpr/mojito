# PROBE (re-probe): a pack-keyed body is checked before anything
# instantiates it.
#
# Both compilers reject both declarations from the template, with nothing
# instantiating them: the pin with "'Ts.values[...]' value has no attribute
# 'nonexistent'", Mojito with "type 'Ts[i]' has no method 'nonexistent'". The
# element under a symbolic index is an opaque dependent type. The enforced
# claims are `assets/type_error/untaken_comptime_if_pack_def.mojo` and
# `untaken_comptime_if_pack_struct.mojo`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_body_untaken_arm.mojo
#         cargo run -- run conformance/probes/pack_body_untaken_arm.mojo
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def poke(self):
        comptime for i in range(Self.Ts.length):
            comptime if i > 100:
                self.storage[i].nonexistent()


def f[*Ts: Writable](*a: *Ts):
    comptime for i in range(a.__len__()):
        comptime if i > 100:
            a[i].nonexistent()


def main():
    print(1)
