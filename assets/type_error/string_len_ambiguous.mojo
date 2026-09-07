# expect: String does not support `__len__`
# Upstream marks `String.__len__` (and `StringSpan.__len__`) `@unavailable`:
# a UTF-8 length is ambiguous between bytes, codepoints, and grapheme
# clusters, so callers spell the unit (`byte_length()`,
# `len(s.codepoints())`, `len(s.graphemes())`).
def main():
    var s = String("hello")
    print(len(s))
