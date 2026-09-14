# expect: value 'c.first' cannot be consumed, because 'c' is used later
# requires: stdlib
# A field moved out inside a branch (`c.first^`) and a sibling read in the
# same branch (`c.tag`).
struct Carrier:
    var first: String
    var second: String
    var tag: Int

    def __init__(out self, first: String, second: String, tag: Int):
        self.first = first
        self.second = second
        self.tag = tag


def taken(var s: String) -> Int:
    return s.byte_length()


def probe(flag: Bool) -> Int:
    var c = Carrier(String("payload"), String("kept"), 3)
    if flag:
        var n = taken(c.first^)
        return n + c.tag
    return c.tag


def main():
    print(probe(True), probe(False))
