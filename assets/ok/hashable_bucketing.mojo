# `Hashable` is a real bound — `hash(key)` (→ `UInt64`) works on an opaque
# `K: Hashable`, and `key.__hash__(hasher)` contributes a key to a caller-owned
# hasher, so a helper can bucket keys. The default hasher is deterministic, so
# equal keys land in the same bucket every run.
from std.hashlib import default_hasher

def bucket_index[K: Hashable](key: K, bucket_count: Int) -> Int:
    return Int(hash(key)) & (bucket_count - 1)

def contributed_bucket[K: Hashable](key: K, bucket_count: Int) -> Int:
    var hasher = default_hasher()
    key.__hash__(hasher)
    return Int(hasher^.finish()) & (bucket_count - 1)

def main():
    var a: Int = 42
    var b: Int = 42
    var c: Int = 7
    # Equal keys bucket identically; the index is always in range.
    print(bucket_index(a, 8) == bucket_index(b, 8))
    print(bucket_index(c, 8) >= 0)
    print(bucket_index("mojo", 16) == bucket_index("mojo", 16))
    # Feeding the key to a hasher directly hashes exactly like `hash(key)`.
    print(contributed_bucket(a, 8) == bucket_index(a, 8))
