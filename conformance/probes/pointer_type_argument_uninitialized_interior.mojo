# PROBE: a `Pointer` type argument naming a local's interior origin.
#
# Mojito prints 3; the pin rejects the application with "use of a
# never-initialized interior reference 'a["element"]'". Mojito accepts a
# program the pin rejects, so this is a divergence.
#
# Observed 2026-09-28 against `Mojo 1.6.0.dev2026092105`.
#
# Run:    mojo run pointer_type_argument_uninitialized_interior.mojo
#         cargo run -- run conformance/probes/pointer_type_argument_uninitialized_interior.mojo
from std.memory.alloc import unsafe_alloc


def main():
    var a = Array[Int, 2](fill=3)
    var p = unsafe_alloc[
        Pointer[Int, origin_of(a)._get_owned_interior["element"]]
    ](1)
    p.unsafe_free()
    print(a[0])
