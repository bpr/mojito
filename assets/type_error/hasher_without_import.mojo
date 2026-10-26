# expect: unknown trait 'Hasher'
# `Hasher` is the bundled `std.hash` trait, which the prelude does not
# export: a bound naming it needs `from std.hash import Hasher`, as at the
# pin. `Hashable` is a prelude name.
@fieldwise_init
struct Key(Copyable, Hashable):
    var id: Int

    def __hash__[H: Hasher](self, mut hasher: H):
        self.id.__hash__(hasher)


def main():
    print(hash(Key(1)) == hash(Key(1)))
