# PROBE (deprecation tracking): does the audited head still ACCEPT the
# transitional `TypeList.size` spelling (with a deprecation warning), or has
# it been removed?
#
# Context: at ae386d1b204, upstream `std/builtin/variadics.mojo` ships
# `comptime size = Self.length` documented as "Deprecated alias for
# `length`" — so Mojito keeps accepting `size` (a bridge, never emitted).
# Re-run this probe at every re-pin; Mojito accepts silently and prints 2.
#
# Run:    mojo run typelist_size_deprecated_alias.mojo
#         cargo run -- run conformance/probes/typelist_size_deprecated_alias.mojo
#
# If Mojo accepts (warning ok): no action — the bridge stands.
# If Mojo rejects: remove Mojito's `size` acceptance (the "length" | "size"
#   arms in src/checker/constraints.rs `constraint_operand` and
#   src/comptime/eval.rs's TypeList member access) and update parity row
#   generics.constraints.
def main():
    comptime tl = TypeList.of[Trait=AnyType, Int, Bool]()
    comptime if tl.size == 2:
        print(2)
