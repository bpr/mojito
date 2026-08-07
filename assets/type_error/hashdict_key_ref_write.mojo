# expect: must be mutable
# HashDict shares `_DictKeyIter`, so its key yields carry the same
# declaration-level immutability.
from std.collections.hashdict import HashDict

def main():
    var h: HashDict[Int, String] = HashDict[Int, String]()
    h[1] = "one"
    for ref k in h:
        k += 1
