# `hash` folds at compile time: the elaborator runs the bundled hashers through
# VM CTFE (`default_comp_time_hasher` is Fnv1a; the default is the keyed
# AHasher) for scalar, Bool, Float64, and string arguments, and each constant
# equals the runtime value.
from std.hashlib import default_comp_time_hasher

comptime CT_INT = hash[default_comp_time_hasher](Int(1))
comptime CT_DEFAULT = hash(Int(1))
comptime CT_STR = hash[default_comp_time_hasher]("hello")
comptime CT_FLOAT = hash(Float64(1.5))
comptime CT_BOOL = hash(True)

def main():
    print(CT_INT)
    print(CT_DEFAULT)
    print(CT_STR)
    print(CT_FLOAT, CT_BOOL)
    print(CT_INT == hash[default_comp_time_hasher](Int(1)), CT_DEFAULT == hash(Int(1)))
