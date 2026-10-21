# PROBE: a `ref` binding of a temporary tuple subject's element is mutable in Mojito; the pin reads the element through `__getitem__` on an rvalue and rejects the write.
#
# The pin rejects it:
#   error: expression must be mutable for in-place operator destination
# while a root `as` binding and a field projection of a temporary stay
# mutable (`(_, _) as t` then `t[0] += "!"` prints `a!`; `W(s=ref x)` then
# `x += "!"` prints `w!`).
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: runs it and prints `a!` until R1239 lands.
#
# Run:    mojo run match_temporary_element_mutable.mojo
#         cargo run -- run conformance/probes/match_temporary_element_mutable.mojo
def main():
    match (String("a"), 1):
        case (ref s, _):
            s += "!"
            print(s)
