# Probe: does a generic struct's method that still keys a clone per instance
# get one for a `StringLiteral` instance?
#
# The pin (2026-09-21) prints `1 0`. Mojito stops with "abort:
# Box.local_ty: unspecialized type-keyed method": `Box("a")` is typed
# `Box[StringLiteral]`, MIR holds the `Int` instance's clone of `local_ty`
# but none for `StringLiteral`, and the call reaches the template's stub.
# `Box(String("a"))` runs. `docs/roadmap.md` names the entry.
struct Box[T: Copyable & Deinitable & Writable]:
    var v: Self.T

    def __init__(out self, var v: Self.T):
        self.v = v^

    def local_ty(self) -> Int:
        comptime U = Self.T
        comptime if U == Int:
            return 1
        return 0


def main():
    print(Box(1).local_ty(), Box("a").local_ty())
