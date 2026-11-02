# PROBE: a function over `Optional[Pointer[Int, _]]`, whose nested `_` hole
# is an auto-parameterized origin, called from one site.
#
# Found at the 2026-11-01 re-pin against `Mojo 1.3.0.dev2026110105
# (ce3afc14)`, which prints `5` (the previous pin rejected the nested hole).
# Mojito prints `None`, a wrong value. With a second call, or a
# `print(Bool(p))` before the read, it prints `5`.
#
# On the fix: move this program to `assets/ok` and delete roadmap R1817.
#
# Run:    mojo run autoparam_nested_hole_single_caller.mojo
#         cargo run -- run conformance/probes/autoparam_nested_hole_single_caller.mojo
def process(p: Optional[Pointer[Int, _]]):
    print(p.value()[])


def main():
    var x = 5
    process(Pointer(to=x))
