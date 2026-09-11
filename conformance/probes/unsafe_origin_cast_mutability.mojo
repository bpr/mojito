# Question: `Pointer.unsafe_origin_cast` with a target origin whose
# mutability differs from the pointer's. Upstream's signature is
# `unsafe_origin_cast[target_origin: Origin[mut=Self.mut]]`, so an
# immutable target (`ImmOrigin(origin_of(x))`) on a mutable
# `Pointer(to=x)` should reject.
#
# Mojito today: accepts and prints `7`. Its `unsafe_origin_cast` rejects
# only the upgrade direction (an immutable pointer cast to a mutable
# target).
#
# On the answer: if Mojo rejects, make Mojito's target check require the
# pointer's own mutability and move this program to `assets/type_error`.
# If Mojo accepts, promote it to an `assets/ok` fixture. Either way delete
# the `unsafe-origin-cast-mutability` bullet from the behavioral-divergences
# list in `docs/roadmap.md`.
def main():
    var x = 7
    var q = Pointer(to=x).unsafe_origin_cast[ImmOrigin(origin_of(x))]()
    print(q[])
