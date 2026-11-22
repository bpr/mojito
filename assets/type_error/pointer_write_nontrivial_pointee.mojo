# `Pointer.write` requires a trivially deinitable pointee: overwriting a
# `String` would leak the old one.
# expect: violated constraint
def main():
    var s = String("a")
    var p = Pointer(to=s)
    p.write(String("b"))
