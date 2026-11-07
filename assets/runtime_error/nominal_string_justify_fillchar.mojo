# A multi-byte fill character aborts on the bundled `_justify`'s check. The
# pinned Mojo's `assert` there runs only under `-D ASSERT=all`; its default
# level skips it and pads with the fill's first byte (R1983).
# expect: abort: fill char needs to be a one byte literal
def main():
    var s = String("hi")
    print(s.ascii_rjust(6, "é"))
