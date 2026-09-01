# `hash` runs the stdlib hashers: `default_hasher` (AHasher) and
# `default_comp_time_hasher` (Fnv1a). Every printed value is current Mojo's
# for the same program; the reflective `Point` feeds its fields in order.
from std.hashlib import default_comp_time_hasher

@fieldwise_init
struct Point(Hashable, Copyable, Movable):
    var x: Int
    var y: Int

def main():
    print("int42", hash(Int(42)))
    print("int0", hash(Int(0)))
    print("intneg1", hash(Int(-1)))
    print("uint7", hash(UInt(7)))
    print("uint123", hash(UInt(123)))
    print("true", hash(True))
    print("uint8_1", hash(UInt8(1)))
    print("f1.5", hash(Float64(1.5)))
    print("fneg2.25", hash(Float64(-2.25)))
    print("zero_fold", hash(Float64(-0.0)) == hash(Float64(0.0)))
    print("int32neg1", hash(Int32(-1)))
    print("hello", hash(String("hello")))
    print("empty", hash(String("")))
    print("mojo", hash(String("mojo")))
    print("abcdefghi", hash(String("abcdefghi")))
    print("fox", hash(String("the quick brown fox jumps over the lazy dog")))
    print("point12", hash(Point(1, 2)))
    print("point00", hash(Point(0, 0)))
    print("fnv_int1", hash[default_comp_time_hasher](Int(1)))
    print("fnv_int42", hash[default_comp_time_hasher](Int(42)))
    print("fnv_f1.5", hash[default_comp_time_hasher](Float64(1.5)))
    print("fnv_hello", hash[default_comp_time_hasher](String("hello")))
    print("fnv_point12", hash[default_comp_time_hasher](Point(1, 2)))
