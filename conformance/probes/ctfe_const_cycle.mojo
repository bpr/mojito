# Probe: a module constant whose initializer calls a function that reads a
# constant derived from the first.
#
# The pin (2026-10-02) rejects it as a cycle in the parameter domain:
# "function instantiation in parameter domain that recursively requires
# itself" / "function recursively calls itself in the parameter domain".
# Mojito rejects it by source order: "VM CTFE failed for 'f': ... Undefined
# variable 'B'". docs/notes/ctfe-request-path.md §Edges and cycles.
def f(n: Int) -> Int:
    return n + B

comptime A = f(1)
comptime B = A + 1

def main():
    print(A)
