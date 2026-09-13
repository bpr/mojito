# A multi-byte fill character aborts. The pinned Mojo has no such assertion:
# it pads with the whole character and prints `ééééhi`, so the abort is a
# Mojito divergence rather than the upstream behaviour it claims.
# expect: abort: fill char needs to be a one byte literal
def main():
    var s = String("hi")
    print(s.ascii_rjust(6, "é"))
