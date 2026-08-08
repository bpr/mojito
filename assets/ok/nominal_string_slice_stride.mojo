# Strided byte-wise slicing on the nominal String, matching the builtin
# literal slice: no raising surface, so main needs no `raises`.
def main():
    var s = String("abcdef")
    print(s[0:6:2])
    print(s[0:6:2] == String("ace"))
    print(s[1::2] == String("bdf"))
