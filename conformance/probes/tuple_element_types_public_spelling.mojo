# PROBE (re-pin tracking): does the audited head keep Tuple's public
# parameter spelled `*Ts` with the pack exposed as the `element_types`
# comptime member?
#
# Context: at ae386d1b204, upstream `std/builtin/tuple.mojo` declares
# `struct Tuple[*Ts: Movable]` with `comptime element_types = Self.Ts` —
# exactly Mojito's spelling (`stdlib/std/collections/tuple.mojo`), so no
# implementation work is open; this probe only watches the public names.
# Re-run at every re-pin; Mojito accepts and prints 2 then 7.
#
# Run:    mojo run tuple_element_types_public_spelling.mojo
#         cargo run -- run conformance/probes/tuple_element_types_public_spelling.mojo
#
# If Mojo matches (accepts, prints 2 then 7): no action.
# If Mojo renames the parameter or member: update
#   stdlib/std/collections/tuple.mojo, the compiler's hardcoded
#   `element_types` sites (src/checker/type_resolution.rs member lookup and
#   src/comptime/specialize.rs `Self.element_types[i]` rewrites), and parity
#   rows types.tuples / generics.constraints.
def main():
    var pair = (3, True)
    print(len(pair))
    var boxed: Tuple[Int] = Tuple[Int](7)
    print(boxed[0])
