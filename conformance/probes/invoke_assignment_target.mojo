# PROBE: a reference-returning call with explicit parameters as an assignment
# target (roadmap R1233).
#
# The pin prints:
#   6
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: a parse error ("invalid assignment target: Invoke").
#
# Run:    mojo run invoke_assignment_target.mojo
#         cargo run -- run conformance/probes/invoke_assignment_target.mojo
struct W:
    var x: Int

    def __init__(out self):
        self.x = 1

    def at[k: Int](mut self) -> ref[self.x] Int:
        return self.x


def main():
    var w = W()
    w.at[0]() = 5
    w.at[0]() += 1
    print(w.x)
