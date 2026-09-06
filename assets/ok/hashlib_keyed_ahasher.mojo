# Upstream's keyed `AHasher[key: U256]`: `default_hasher` is the zero-keyed
# specialization, `hash_seeded` folds a seed into the key, and a zero seed is
# the default hasher. Every value is current Mojo's for the same program.
from std.hashlib._ahash import U256, hash_seeded

def main():
    var text = String("hello")
    print(hash(text))
    print(hash_seeded(text, U256(1, 2, 3, 4)))
    print(hash_seeded(text, U256(0)) == hash(text))
    print(hash_seeded(Int(42), U256(0)) == hash(Int(42)))
