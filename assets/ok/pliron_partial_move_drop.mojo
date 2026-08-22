# Per-leaf presence flags: moving one field out leaves the rest to drop
# normally, and any moved leaf suppresses whole-value destructor work — the
# VM's tombstone rule. The conditional move makes the surviving-leaf set
# dynamic and the early return exercises cleanup-edge drops.
struct Carrier:
    var first: String
    var second: String
    var tag: Int

    def __init__(out self, first: String, second: String, tag: Int):
        self.first = first
        self.second = second
        self.tag = tag


def taken(var s: String) -> Int:
    return len(s)


def probe(flag: Bool) -> Int:
    var c = Carrier(String("payload"), String("kept"), 3)
    if flag:
        var n = taken(c.first^)
        return n + c.tag
    return c.tag


def main():
    print(probe(True), probe(False))
