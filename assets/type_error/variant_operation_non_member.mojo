# expect: Type does not exist in Variant
from std.utils import Variant

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    print(v.isa[Float64]())
