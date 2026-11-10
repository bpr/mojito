# Upstream's string iterators by name: `CodepointSliceIter`,
# `CodepointsIter`, and `GraphemeSliceIter` from
# `std.collections.string.iterators`, re-exported by the package, with
# `forward` choosing the direction `__next__` takes, and
# `next`/`next_back`/`peek_back` from either end.
from std.collections.string import CodepointsIter, GraphemeSliceIter
from std.collections.string.iterators import CodepointSliceIter


def backwards() -> GraphemeSliceIter[ImmStaticOrigin, False]:
    return "abé".graphemes_reversed()


def slices(s: String) -> Int:
    var it: CodepointSliceIter[origin_of(s)] = s.codepoint_slices()
    var n = 0
    for _ in it:
        n += 1
    return n


def main():
    for g in backwards():
        print(g)
    var lit: CodepointSliceIter[ImmStaticOrigin, False] = (
        "xyz".codepoint_slices_reversed()
    )
    print(len(lit), lit.peek_back().value(), lit.next().value())
    for c in lit:
        print(c)

    var s = String("héllo")
    print(slices(s))
    var cs = s.codepoint_slices()
    print(cs.next().value(), cs.next_back().value(), len(cs))
    print(cs.peek_next().value(), cs.peek_back().value())

    var cps: CodepointsIter[origin_of(s)] = s.codepoints()
    print(cps.next().value(), len(cps))
    for cp in cps:
        print(Int(cp))

    var gs = StringSpan(s).graphemes()
    print(gs.remaining_byte_length(), len(gs))
    print(gs.next_back().value(), gs.next().value(), gs.peek_back().value())
    print(gs.remaining_byte_length())
    for g in gs:
        print(g)
