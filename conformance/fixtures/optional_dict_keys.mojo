# Optional is Hashable when its element is, so `Dict[Optional[Int], _]` keys
# work with `None`, an implicitly converted Int, and an explicit Optional.
# (Kept out of assets/ok: the native backend still mis-handles a generic
# struct temporary holding an implicitly-copyable heap-owning field when it
# is appended to a List — Dict's entry — see the roadmap's native residue.)
def main() raises:
    var table = Dict[Optional[Int], String]()
    table[None] = "none"
    table[1] = "one"
    table[Optional[Int](2)] = "two"
    print(len(table), table[None], table[1], table[Optional[Int](2)])
    print(Optional[Int](2) in table, Optional[Int](3) in table, None in table)
