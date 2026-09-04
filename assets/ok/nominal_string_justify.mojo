# Byte-width justification with a one-byte fill character: a string already
# at least `width` bytes long is returned unchanged, and center puts the
# extra fill byte on the right.
def main():
    var hi = String("hi")
    print("[", hi.ascii_rjust(5), "]", "[", hi.ascii_ljust(5), "]", "[", hi.ascii_center(5), "]")
    print("[", hi.ascii_center(6), "]", "[", String("hello").ascii_center(3), "]", "[", hi.ascii_rjust(5, "*"), "]")
    print("[", String("odd").ascii_center(8, "-"), "]", "[", String("é").ascii_ljust(3, "."), "]")
    print(hi.ascii_rjust(2) == "hi", hi.ascii_ljust(0).byte_length(), hi.ascii_center(9, "=").byte_length())
