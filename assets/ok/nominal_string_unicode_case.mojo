# upper/lower over the full Unicode 16 tables generated from upstream's
# lookups (`scripts/gen-string-tables`): simple mappings across scripts,
# the SpecialCasing multi-codepoint uppers (`ß` -> `SS`, `ﬁ` -> `FI`,
# `ﬃ` -> `FFI`, `ŉ` -> `ʼN`, `և` -> `ԵՒ`, `ΐ` -> `Ϊ́`), titlecase
# digraphs that map both ways, and the isupper/islower rule (at least one
# cased character, none of the other case).
def main():
    print(String("Hello, World! 123").upper(), String("Hello, World! 123").lower())
    print(String("éàü ÿ ß").upper(), String("ÉÀÜ Ÿ").lower())
    print(String("αβγ ς σ").upper(), String("ΑΒΓ Σ").lower())
    print(String("дом ёж").upper(), String("ДОМ ЁЖ").lower())
    print(String("straße ﬁne ﬃ ŉ").upper())
    print(String("ǆ ǅ Ǆ").upper(), String("ǆ ǅ Ǆ").lower())
    print(String("և ΐ ᾳ ǰ").upper())
    print(String("ı İ").upper(), String("ı İ").lower())
    print(String("ქართული").upper(), String("Ⴀ Ꭰ ꭰ").lower())
    print(String("ǅ").isupper(), String("ǅ").islower(), String("ﬁ").islower(), String("ﬁ").isupper())
    print(String("ÉΣД").isupper(), String("éσд").islower(), String("ß").islower(), String("ẞ").isupper())
    print(String("A1!").isupper(), String("123").isupper(), String("").islower())
