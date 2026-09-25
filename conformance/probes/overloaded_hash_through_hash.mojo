# Pin gap probe (Mojo 1.2.0.dev2026092105): `hash(x)` of a struct that
# overloads `__hash__` on the hasher's type. The pin prints True; Mojito
# checks the program and then stops at run time with "vm: unknown method
# 'Twin.__hash__'": the call reaches the VM under the method's plain name,
# which no member of the overload set is lowered under. Roadmap section 3 carries the entry.
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
