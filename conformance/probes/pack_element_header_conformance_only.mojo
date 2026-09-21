# PROBE (re-probe): a struct-header conditional conformance does not license
# an element use inside the method that implements it.
#
# The pin rejects: "invalid call to 'write': an element of 'args' with type
# 'Ts.values[...]' does not conform to trait 'Writable'; either prove the
# conformance with 'conforms_to', or add conformance". The method needs its
# own `where`. Mojito rejects likewise; the enforced claim is
# `assets/type_error/pack_element_missing_bound.mojo`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_element_header_conformance_only.mojo
#         cargo run -- run conformance/probes/pack_element_header_conformance_only.mojo
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
    Writable where Ts.all_conforms_to[Writable](),
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def write_to(self, mut writer: Some[Writer]):
        comptime for i in range(Self.Ts.length):
            writer.write(self.storage[i])


def main():
    print(1)
