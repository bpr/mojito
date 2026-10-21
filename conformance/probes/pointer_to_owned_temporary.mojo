# PROBE: `Pointer(to=)` an owned call result (roadmap R1234).
#
# The pin materializes the temporary and keeps it alive until the pointer's
# last use. It prints:
#   after
#   3
#   deinit 3
#   end
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects it ("Pointer(to=...) requires a place expression").
#
# Run:    mojo run pointer_to_owned_temporary.mojo
#         cargo run -- run conformance/probes/pointer_to_owned_temporary.mojo
struct D(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("deinit", self.v)


def make() -> D:
    return D(3)


def main():
    var p = Pointer(to=make())
    print("after")
    print(p[].v)
    print("end")
