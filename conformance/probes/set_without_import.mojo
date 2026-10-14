# PROBE: `Set` spelled without an import.
#
# The pin rejects it: "use of unknown declaration 'Set'; did you mean to
# import it from 'std.collections'?". Mojito's prelude exports `Set`, so it
# accepts the program and prints 1.
#
# Observed 2026-10-13 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run set_without_import.mojo
#         cargo run -- run conformance/probes/set_without_import.mojo
def main():
    var s = Set[Int]()
    s.add(1)
    print(len(s))
