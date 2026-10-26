# Pin gap probe (Mojo 1.2.0.dev2026092105): a `mut self` method as the
# witness of `Hashable`'s read-`self` `__hash__` requirement. The pin accepts
# the conformance and prints True; Mojito rejects it ("struct 'Odd' method
# '__hash__' does not match the signature required by trait 'Hashable'"),
# because a witness must repeat the requirement's receiver convention
# exactly. docs/roadmap.md R67.
from std.hash import Hasher


struct Odd(Copyable, Hashable, Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __hash__(mut self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)


def main():
    var odd = Odd(1)
    print(hash(odd) == hash(Odd(1)))
