# A `Variant` may repeat an alternative: an operation selects the first
# position holding its type, as `_get_type_index` does.
from std.utils import Variant

def main():
    var v = Variant[Int, Int](3)
    print(v.isa[Int](), v[Int])
