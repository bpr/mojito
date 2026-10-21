# PROBE: a `comptime for` target destructures each element, as a runtime `for` target does.
#
# The pin runs it and prints:
#   1 2
#
# Mojito: a parse error, "a destructuring 'comptime for' target is not
# supported", since `ComptimeFor` binds one name (R1228).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run comptime_for_destructuring_target.mojo
#         cargo run -- run conformance/probes/comptime_for_destructuring_target.mojo
def main():
    comptime for a, b in [(1, 2)]:
        print(a, b)
