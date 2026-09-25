# Pin gap probe (Mojo 1.2.0.dev2026092105): a `mut self` method as the
# witness of `Hashable`'s read-`self` `__hash__` requirement. The pin accepts
# the conformance and prints True; Mojito rejects it ("type 'Odd' for
# parameter 'Self' does not conform to trait 'Hashable': missing required
# operation"), because a witness must repeat the requirement's receiver
# convention exactly. Roadmap section 3 carries the entry.
from std.hashlib import Hasher


struct Odd(Copyable, Hashable, Movable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __hash__(mut self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)


def main():
    var odd = Odd(1)
    print(hash(odd) == hash(Odd(1)))
