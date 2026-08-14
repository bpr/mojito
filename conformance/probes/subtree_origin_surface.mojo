# PROBE (surface + diagnostics pinning): where does the audited head accept
# the experimental `._subtree` origin projection, and with what diagnostics?
# Specifically: (1) is `origin._subtree` legal in `ref [...]` result/parameter
# clauses (Mojito rejects it there — first-pass surface is Pointer origin
# arguments and origin_cast targets only); (2) what does the head report for
# `._subtree._subtree` and for a projection below `._subtree` (Mojito:
# "'_subtree' is a terminal origin projection"); (3) what is the head's
# second-write diagnostic for a mutable subtree reference (Mojito rejects the
# use after the first write as an invalidated interior reference naming the
# write site)?
#
# Context: nightly §7. Upstream exposes `Origin._subtree` "for internal
# experimentation" (#lit.origin.subtree) with no stdlib users, so the legal
# positions and diagnostics have no public test evidence. Mojito's acceptance
# lives in src/checker/type_resolution.rs (pointer_origin_arg/expr,
# append_subtree); the ref-clause rejection in src/checker/origins.rs.
#
# Run:    mojo run subtree_origin_surface.mojo
#         cargo run -- run conformance/probes/subtree_origin_surface.mojo
#
# If Mojo accepts `ref [o._subtree]` clauses: widen Mojito's surface (drop
#   reject_subtree_origin_here for that position) and pin the semantics.
# If Mojo rejects the whole `._subtree` spelling outside compiler internals:
#   record Mojito's Pointer/origin_cast acceptance as a deliberate bridge in
#   parity row ownership.origins-advanced.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).origin_cast[
            origin_of(self)._subtree
        ]()

def main():
    var b = Buf(3)
    var p = b.view()
    p[] = 4
    print(b.value)
