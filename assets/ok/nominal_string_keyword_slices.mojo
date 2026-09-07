# Strict keyword slices on the nominal String and its borrowed StringSpan
# views: `s[byte=a:b]` (endpoints on UTF-8 boundaries) and
# `s[codepoint=a:b]` return sub-views of the String's buffer; a StringSpan
# also slices by grapheme (which String itself does not) and sub-slices by
# byte. Omitted bounds are preserved.
def main() raises:
    var s = String("héllo🙂")
    var b = s[byte=0:1]
    print(b, b.byte_length())
    var full = s[byte=:]
    print(full.byte_length())
    var cp = s[codepoint=1:3]
    print(cp, cp.byte_length())
    var sub = cp[byte=0:2]
    print(sub)
    var t = String("héllo🙂")
    var sp = StringSpan(t)
    print(sp.byte_length(), sp.count_graphemes(), sp.count_codepoints())
    var g = sp[grapheme=5:6]
    print(g, g.byte_length())
    var head = sp[codepoint=:2]
    print(head)
    print(sp[byte=0], g[grapheme=0])
