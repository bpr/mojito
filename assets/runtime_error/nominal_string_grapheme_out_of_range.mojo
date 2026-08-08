# expect: grapheme index out of range
def main() raises:
    var s = String("\U0001f1fa\U0001f1f8")
    print(s[grapheme=1])
