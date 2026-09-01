# expect: __hash__
# `Hashable` no longer contributes a value-returning `__hash__()`: the
# bounded operation takes the hasher to feed.
def bucket[K: Hashable](key: K) -> UInt:
    return key.__hash__()

def main():
    print(bucket(1))
