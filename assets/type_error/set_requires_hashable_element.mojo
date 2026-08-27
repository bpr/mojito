# expect: no constructor overload matches
# Set requires a Hashable element: upstream's KeyElement bound (Equatable &
# Hashable & Movable) and Mojito's T bound both reject an element type
# without hash support. (Set imported explicitly — required upstream;
# Mojito's prelude visibility is a recorded divergence.)
from std.collections.set import Set

@fieldwise_init
struct Elem(Equatable, Copyable, Movable):
    var value: Int

    def __eq__(self, other: Self) -> Bool:
        return self.value == other.value

def main():
    var values: Set[Elem] = Set[Elem]()
    values.add(Elem(1))
