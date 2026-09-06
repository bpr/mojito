# Container `Hashable` on the hasher protocol, in upstream's bodies: List and
# Array feed each element (no length prefix), Set XORs the default hasher's
# element hashes order-independently, Dict mixes per-entry hashes under the
# caller's hasher type, and Optional/Variant/Tuple tag then delegate. Every
# value is current Mojo's for the same program.
from std.collections import Set
from std.hashlib import default_comp_time_hasher
from std.utils import Variant

def main():
    var xs: List[Int] = [1, 2, 3]
    print("list123", hash(xs))
    print("list_empty", hash(List[Int]()))
    var ss: List[String] = ["a", "b"]
    print("list_ab", hash(ss))
    var ys: List[Int] = [1, 2]
    print("fnv_list12", hash[default_comp_time_hasher](ys))
    var arr: Array[Int, 3] = [1, 2, 3]
    print("array123", hash(arr))
    print("list_eq_array", hash(xs) == hash(arr))
    var s: Set[Int] = {1, 2, 3}
    print("set123", hash(s))
    var d: Dict[String, Int] = {"a": 1, "b": 2}
    print("dict_ab", hash(d))
    var e: Dict[String, Int] = {"b": 2, "a": 1}
    print("dict_order_independent", hash(d) == hash(e))
    print("opt3", hash(Optional[Int](3)))
    print("opt_none", hash(Optional[Int]()))
    print("var_int7", hash(Variant[Int, String](7)))
    print("var_str7", hash(Variant[Int, String](String("7"))))
    print("tuple", hash((1, True)))
