# expect: no constructor overload matches
# Tuple's Hashable conformance is conditional on every element: a tuple whose
# element type lacks `__hash__` cannot satisfy Dict's Hashable key bound.
@fieldwise_init
struct Opaque(Equatable, Copyable, Movable):
    var value: Int

    def __eq__(self, other: Self) -> Bool:
        return self.value == other.value

def main():
    var table = Dict[Tuple[Int, Opaque], Int]()
    table[(1, Opaque(2))] = 3
