# PROBE: a subscript of a list literal in a runtime position.
#
# The pin prints 1 and b; Mojito stops with "reference binding to a non-place
# expression", since `List.__getitem__` borrows its receiver and a literal is
# no place. A call result (`make()[1]`) is materialized and works.
#
# Observed 2026-10-04 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run list_literal_subscript.mojo
#         cargo run -- run conformance/probes/list_literal_subscript.mojo
def main():
    print([1, 2][0])
    print(["a", "b"][1])
