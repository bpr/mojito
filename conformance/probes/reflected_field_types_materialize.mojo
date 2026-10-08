# PROBE: a reflected field-type list materialized whole.
#
# The pin prints 2 twice: `field_types()` is a zero-sized `TypeList` whose
# length is the field count. Mojito rejects `materialize[types]()` with
# "type-valued or symbolic comptime values cannot materialize at runtime",
# over a closed subject and over a type parameter alike.
#
# Observed 2026-10-08 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run reflected_field_types_materialize.mojo
#         cargo run -- run conformance/probes/reflected_field_types_materialize.mojo
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


def show[T: AnyType]():
    comptime types = reflect[T].field_types()
    var all = materialize[types]()
    print(len(all))


def main():
    comptime types = reflect[Point].field_types()
    var all = materialize[types]()
    print(len(all))
    show[Point]()
