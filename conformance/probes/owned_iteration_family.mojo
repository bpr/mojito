# PROBE (family pinning): which collections declare consuming iteration
# (`IterableOwned`) at the audited head? Mojito declares it on List, Array,
# and Optional (all bounded `Movable & Deinitable`); Dict, Set, and
# StringDict do not declare owned iteration; Tuple uses
# `consume_elements`/`deinit_with` instead.
#
# Run:    mojo run owned_iteration_family.mojo
#         cargo run -- run conformance/probes/owned_iteration_family.mojo
#
# For each collection the head iterates consumingly that Mojito rejects:
#   add the owned iterator to the bundled declaration (the List/Array
#   pattern) and extend parity row control.loops.
def main():
    var s: Set[Int] = {1, 2}
    for var element in s^:
        print("set", element)
