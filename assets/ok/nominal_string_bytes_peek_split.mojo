# `bytes()` iterates the raw UTF-8 bytes (Sized), `peek_next()` on the
# codepoint iterators returns the next element without advancing,
# `split_at_grapheme(n)` yields the views before and after the n-th
# cluster boundary (read as a pair; destructuring a returned Tuple of
# views is a recorded VM residue), and the codepoint/grapheme counts never
# raise, even over a buffer that is not valid UTF-8.
def main():
    var s = String("héllo🙂")
    var total = 0
    for b in s.bytes():
        total += Int(b)
    print(total, len(s.bytes()))
    var view = StringSpan(s)
    var seen = 0
    for b in view.bytes():
        seen += 1
    print(seen)
    var it = s.codepoint_slices()
    var first = it.peek_next()
    var again = it.peek_next()
    print(first.value(), again.value(), len(it))
    for piece in it:
        print(piece)
        break
    var cps = s.codepoints()
    var cp = cps.peek_next()
    print(cp.value(), Int(cp.value()), len(cps))
    var empty = String("").codepoints()
    var none = empty.peek_next()
    print(Bool(none), Bool(String("").codepoint_slices().peek_next()))
    var pair = s.split_at_grapheme(2)
    print(pair[0], pair[1])
    var zero = s.split_at_grapheme(0)
    print(zero[0].byte_length(), zero[1])
    var past = view.split_at_grapheme(99)
    print(past[0], past[1].byte_length())
    var family = String("👨‍👩‍👧x")
    var cut = family.split_at_grapheme(1)
    print(cut[0].byte_length(), cut[1])
    var bad = String(unsafe_uninit_length=3)
    var ptr = bad.unsafe_ptr_mut()
    ptr[0] = Byte(0xE2)
    ptr[1] = Byte(0x80)
    ptr[2] = Byte(0x41)
    print(bad.count_codepoints(), bad.count_graphemes(), StringSpan(bad).count_codepoints())
