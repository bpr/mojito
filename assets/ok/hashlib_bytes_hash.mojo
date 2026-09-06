# Upstream's raw-bytes hashing entry points: `hash(bytes, n)` over an
# `ImmPointer[UInt8, _]` (any provenance, immutable) agrees with hashing the
# String those bytes spell, and `hash_seeded_bytes` with `hash_seeded`.
from std.hashlib._ahash import U256, hash_seeded, hash_seeded_bytes

def main():
    var s = String("hello")
    print(hash(s.unsafe_ptr(), 5))
    print(hash(s.unsafe_ptr(), 5) == hash(s))
    print(hash_seeded_bytes(s.unsafe_ptr(), 5, U256(1, 2, 3, 4)))
    print(hash_seeded_bytes(s.unsafe_ptr(), 5, U256(1, 2, 3, 4)) == hash_seeded(s, U256(1, 2, 3, 4)))
