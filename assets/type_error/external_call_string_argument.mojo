# expect: argument 1 to external_call["strlen"]
# A `String` is not a C string: as upstream, the caller spells
# `.as_c_string_slice()` (or passes a byte pointer).
from std.ffi import c_size_t, external_call


def main():
    var text = String("hello")
    print(external_call["strlen", c_size_t](text))
