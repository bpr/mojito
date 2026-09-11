# `__reversed__()` / `graphemes_reversed()` walk the extended grapheme
# clusters back to front (CR LF, ZWJ sequences, regional-indicator pairs,
# and combining marks stay whole), `codepoint_slices_reversed()` walks the
# codepoints, both serve views too, and every reversed iterator is Sized.
def main():
    var s = String("héllo🙂")
    for g in s.__reversed__():
        print(g)
    var family = String("a\r\n👨‍👩‍👧b🇫🇷🇩🇪é")
    var count = 0
    for g in family.graphemes_reversed():
        print(g.byte_length())
        count += 1
    print(count, family.count_graphemes(), len(family.graphemes_reversed()))
    var pieces = 0
    for piece in s.codepoint_slices_reversed():
        pieces += piece.byte_length()
    print(pieces, len(s.codepoint_slices_reversed()))
    var view = StringSpan(s)
    for g in view.__reversed__():
        print(g)
    for g in view.graphemes_reversed():
        print(g.byte_length())
    var family_view = StringSpan(family)
    for g in family_view.__reversed__():
        print(g.byte_length())
        break
    var marks = String("éẍ")
    for g in marks.graphemes_reversed():
        print(g.byte_length(), g.count_codepoints())
