# The audited head rejects `len(String/StringSlice)` (byte vs codepoint vs
# grapheme ambiguity; use `byte_length()`/`len(s.codepoints())`/…); Mojito
# still accepts byte-length `len` — a recorded acceptance divergence.
def main():
    var s = String("hello")
    print(len(s))
