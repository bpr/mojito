# `unsafe_get[T]()` is the method spelling of the checked projection read
# (Mojito validates the tag deterministically instead of upstream's debug
# assert), and `is_type_supported[T]()` also dispatches on the parameterized
# type itself (upstream's static spelling).
from std.utils import Variant

def main():
    var v = Variant[Int, String](7)
    print(v.unsafe_get[Int](), v.unsafe_get[Int]() + 1)
    var s = Variant[Int, String](String("mojo"))
    print(s.unsafe_get[String]().byte_length())
    print(Variant[Int, String].is_type_supported[Int](), Variant[Int, String].is_type_supported[Float64]())
    print(v.is_type_supported[String]())
