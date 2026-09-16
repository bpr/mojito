# Optional is Hashable when its element is, so `Dict[Optional[Int], _]` keys
# work with `None`, an implicitly converted Int, and an explicit Optional.
def main() raises:
    var table = Dict[Optional[Int], String]()
    table[None] = "none"
    table[1] = "one"
    table[Optional[Int](2)] = "two"
    print(len(table), table[None], table[1], table[Optional[Int](2)])
    print(Optional[Int](2) in table, Optional[Int](3) in table, None in table)
