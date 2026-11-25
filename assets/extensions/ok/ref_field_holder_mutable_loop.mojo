# A mutable loop over a local holder's `ref` field, directly and through a
# `ref` binding of the holder. The loop's mutable loan on the list meets the
# holder's loan, which excludes nothing, so both loops write through: `2`,
# then `3 4` (R1160).
struct Walk[o: MutOrigin]:
    var src: ref[Self.o] List[Int]

    def __init__(out self, ref[Self.o] l: List[Int]):
        self.src = l


def main():
    var l: List[Int] = [1, 2]
    var w = Walk(l)
    for ref x in w.src:
        x += 1
    print(w.src[0])
    ref r = w
    for ref x in r.src:
        x += 1
    print(r.src[0], l[1])
