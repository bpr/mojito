# expect: `len(String/StringSlice)` is not supported
# Upstream's `len` declares an `@unavailable` overload over `StringSlice`:
# a UTF-8 length is ambiguous between bytes, codepoints, and grapheme
# clusters, so callers spell the unit (`byte_length()`,
# `len(s.codepoints())`, `len(s.graphemes())`).
def main():
    var s = String("hello")
    print(len(s))
