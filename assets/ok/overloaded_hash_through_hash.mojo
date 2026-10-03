# `hash(x)` of a struct that overloads `__hash__` on the hasher's type: the
# bundled `hash` is a plain trait-bound `def`, served by its template, and the
# elaborator selects the overload the hasher instance takes. The pin prints
# `True`.
from std.hashlib import Hasher
from std.hashlib._ahash import AHasher


@fieldwise_init
struct Twin(Copyable, Deinitable, Hashable, Movable):
    var x: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)

    def __hash__(self, mut hasher: AHasher[SIMD[DType.uint64, 4](0)]):
        self.x.__hash__(hasher)


def main():
    print(hash(Twin(1)) == hash(Twin(1)))
