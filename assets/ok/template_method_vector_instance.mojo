# A struct instance over a multi-lane vector (`Box[SIMD[DType.float32, 2]]`)
# derives its method's hashed leaf from the checked template, and its
# per-instantiation constructor clone keeps the whole instance name natively:
# the vector's `DType.float32` never splits the method symbol.
from std.hashlib import Hasher
from std.hashlib._ahash import AHasher


struct Box[T: Copyable & Deinitable & Hashable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def digest[H: Hasher](self, mut hasher: H):
        self.value.__hash__(hasher)


def main():
    var hasher = AHasher[SIMD[DType.uint64, 4](0)]()
    Box[SIMD[DType.float32, 2]](SIMD[DType.float32, 2](1.0, 2.0)).digest(hasher)
    print(hasher^.finish())
