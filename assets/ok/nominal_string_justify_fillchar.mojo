# A multi-byte fill character pads with its first byte: `_justify`'s
# one-byte `assert` runs only under `-D ASSERT=all`, so the default level
# skips it, as at the pin.
def main():
    var s = String("hi")
    var r = s.ascii_rjust(6, "é")
    print(r.byte_length())
    var b = r.as_bytes()
    print(Int(b[0]), Int(b[3]), Int(b[4]), Int(b[5]))
    print(String("ab").ascii_ljust(3, "é").byte_length())
