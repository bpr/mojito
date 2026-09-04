# codepoints() yields decoded Codepoint values, codepoint_slices() borrowed
# one-codepoint views, graphemes() the grapheme-cluster views ordinary
# iteration yields; all three are Sized.
def main() raises:
    var src = String("héllo")
    var count = 0
    for cp in src.codepoints():
        count += Int(cp)
    print(count)
    for piece in src.codepoint_slices():
        print(piece, piece.byte_length())
    var gx = String("éx")
    for g in gx.graphemes():
        print(g.byte_length())
    print(len(src.codepoints()))
    print(len(src.codepoint_slices()), len(gx.graphemes()))
    var view = StringSpan(src)
    for cp in view.codepoints():
        print(cp, cp.is_ascii())
    for piece in view.codepoint_slices():
        print(piece.byte_length())
