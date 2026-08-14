# expect: has no field 'take'
from std.utils import Variant

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    print(v.take[Int]())
