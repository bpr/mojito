# PROBE: `__len__` spelled as a method on a specialized runtime pack.
#
# **Differs.** The pin prints 2. Mojito's clone types `b.__len__()` and the
# VM then refuses it: `internal tuple-pack storage has no runtime method
# '__len__'; public Tuple methods require nominal lowering`. `len(b)` runs.
# Filed in `docs/roadmap.md` §1. When Mojito runs it, promote this file to
# `assets/ok/runtime_pack_dunder_len.mojo` with its manifest rows.
#
# Observed 2026-09-22 against `Mojo 1.2.0.dev2026092105`.
#
# Run:    mojo run runtime_pack_dunder_len.mojo
#         cargo run -- run conformance/probes/runtime_pack_dunder_len.mojo
def sink[*Ts: Copyable](*b: *Ts) -> Int:
    return b.__len__()


def main():
    print(sink(1, True))
