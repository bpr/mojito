# A string literal payload materializes as `String`, which selects the
# `String` alternative.
from std.utils import Variant

def main():
    var v = Variant[Int, String]("x")
    print(v.isa[String](), v[String])
