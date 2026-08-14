# PROBE (API-shape pinning): OwnedPointer's surface at the audited head —
# the constructor set (value, `init_with=`?), `into_inner`, the borrowed
# dereference spelling (`p[]`? Mojito reserves the empty subscript for raw
# pointers — a recorded subset gap), `unsafe_ptr`'s existence/signature,
# and prelude visibility (Mojito keeps it import-only in std.memory).
#
# Run:    mojo run owned_pointer_surface.mojo
#         cargo run -- run conformance/probes/owned_pointer_surface.mojo
#
# If `p[]` is the only borrowed access: record the empty-subscript nominal
#   dispatch as the blocking gap in parity; if prelude-visible, export it
#   from stdlib/std/prelude.mojo; adjust stdlib/std/memory.mojo names.
from memory import OwnedPointer

def main():
    var p = OwnedPointer[Int](41)
    print(p[])
    print(p^.into_inner())
