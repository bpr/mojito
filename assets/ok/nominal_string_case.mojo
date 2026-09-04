# upper/lower over the bundled simple-case subset (ASCII, Latin-1, Latin
# Extended-A, Greek, Cyrillic; `ß` uppercases to `SS`), and the
# isupper/islower rule: at least one cased character and none of the other
# case.
def main():
    var mixed = String("Hello, World! 123")
    print(mixed.upper(), mixed.lower())
    var latin = String("éàü ÿ ß")
    print(latin.upper(), String("ÉÀÜ Ÿ").lower())
    print(String("αβγ ς σ").upper(), String("ΑΒΓ Σ").lower())
    print(String("дом ёж").upper(), String("ДОМ ЁЖ").lower())
    print(String("āĺź").upper(), String("ĀĹŹ").lower())
    print(String("ABC").isupper(), String("AbC").isupper(), String("A1!").isupper(), String("123").isupper(), String("").isupper())
    print(String("abc").islower(), String("aBc").islower(), String("a1!").islower(), String("123").islower(), String("").islower())
    print(String("ÉΣД").isupper(), String("éσд").islower(), String("ß").islower())
