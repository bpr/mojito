# Codepoint.from_u32 constructs directly from a scalar (Mojito is
# Int-based): the character text is UTF-8-encoded in ordinary library code
# through runtime Byte(Int) conversions, across all four sequence widths.
# Negatives, surrogates, and values beyond U+10FFFF are absent.
def main() raises:
    var q: String = "?"
    var fallback = q[codepoint=0]
    var g = Codepoint.from_u32(103).or_else(fallback)
    print(g)
    print(Int(g))
    var e_acute = Codepoint.from_u32(0xE9).or_else(fallback)
    print(e_acute)
    print(e_acute.utf8_byte_length())
    var snowman = Codepoint.from_u32(0x2603).or_else(fallback)
    print(snowman)
    var emoji = Codepoint.from_u32(0x1F600).or_else(fallback)
    print(emoji)
    print(emoji.utf8_byte_length())
    print(Codepoint.from_u32(0xD800).is_some())
    print(Codepoint.from_u32(-1).is_some())
    print(Codepoint.from_u32(0x110000).is_some())
