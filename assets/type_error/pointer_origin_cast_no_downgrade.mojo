# expect: an origin cast keeps the Pointer's capability
# Upstream spells the cast `unsafe_origin_cast[target_origin: Origin[mut=Self.mut]]`,
# so the target's mutability must equal the pointer's. An immutable target on a
# mutable pointer is rejected for the same reason the upgrade direction is
# (`pointer_origin_cast_no_upgrade.mojo`).
def main():
    var x = 7
    var q = Pointer(to=x).unsafe_origin_cast[ImmOrigin(origin_of(x))]()
    print(q[])
