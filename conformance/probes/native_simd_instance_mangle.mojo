# A struct instance whose argument is a multi-lane vector
# (`Box[SIMD[DType.float32, 2]]`) runs on the VM, where its method derives the
# hashed leaf from the checked template, but the native backend refuses its
# constructor: the call binding for `Box$mono$TSIMD$DType$float32$$2$.float32,
# 2]` "disagrees with its compiled arity". The mangled instance name keeps a
# fragment of the unmangled type argument. Filed from the vector `__hash__`
# work (roadmap section 2); the pinned Mojo runs it.
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
