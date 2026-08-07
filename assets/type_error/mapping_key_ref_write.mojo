# expect: must be mutable
# Mapping key yields are declaration-level immutable (the hash invariant owns
# the key), so a `for ref` write through a key binding rejects even over a
# mutable Dict.
def main():
    var d: Dict[Int, Int] = Dict[Int, Int]()
    d[1] = 10
    for ref k in d:
        k += 1
