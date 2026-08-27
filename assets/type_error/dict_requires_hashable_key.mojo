# expect: no constructor overload matches
# Dict requires a Hashable key: upstream's KeyElement bound (Equatable &
# Hashable & Movable) and Mojito's K bound both reject a key type without
# hash support.
@fieldwise_init
struct Key(Equatable, Copyable, Movable):
    var value: Int

    def __eq__(self, other: Self) -> Bool:
        return self.value == other.value

def main():
    var values: Dict[Key, Int] = Dict[Key, Int]()
    values[Key(1)] = 2
