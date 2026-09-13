# expect: an origin cast keeps the Pointer's capability
# Upstream spells the cast `unsafe_origin_cast[target_origin: Origin[mut=Self.mut]]`:
# an immutable target cannot stand in for a mutable pointer's origin. The other
# direction is accepted but keeps the immutable capability
# (`pointer_origin_cast_no_upgrade.mojo`).
def main():
    var x = 7
    var q = Pointer(to=x).unsafe_origin_cast[ImmOrigin(origin_of(x))]()
    print(q[])
