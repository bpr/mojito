# PROBE (divergence): two type-pack `__init__` overloads cannot be constructed.
#
# The checker selects the second constructor, as the pin does, and the VM then
# reports "checked constructor 'H.__init__$ov$Int$Int$$u2A$Ts$Writable' is
# missing from MIR". The rejection is safe, but the pin runs the program.
#
# Observed 2026-09-19 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    3
#   mojito: unsupported feature: vm: checked constructor ... is missing from MIR
#
# Run:    mojo run pack_overload_constructor_missing_mir.mojo
#         cargo run -- run conformance/probes/pack_overload_constructor_missing_mir.mojo
#
# When fixed: move this file to `assets/ok/`.
struct H:
    var k: Int

    def __init__[*Ts: Writable](out self, a: Int, *rest: *Ts):
        self.k = 2

    def __init__[*Ts: Writable](out self, a: Int, b: Int, *rest: *Ts):
        self.k = 3


def main():
    var x = 1
    print(H(x, x, x).k)
