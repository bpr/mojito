# PROBE: an `IsTrivially*` predicate spelled without an import.
#
# The pin rejects it: "use of unknown declaration 'IsTriviallyCopyable'; did
# you mean to import it from 'std.traits'?". Mojito's checker recognizes the
# spelling as a builtin predicate (`types::trivial_predicate_name`) whatever
# is in scope, so it accepts the program and prints 1.
#
# Observed 2026-10-24 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run is_trivially_without_import.mojo
#         cargo run -- run conformance/probes/is_trivially_without_import.mojo
def main():
    comptime if IsTriviallyCopyable[Int]:
        print(1)
    else:
        print(2)
