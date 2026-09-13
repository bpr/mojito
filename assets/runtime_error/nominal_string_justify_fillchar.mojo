# A multi-byte fill character aborts on upstream's own `_justify` assertion.
# The pinned Mojo checks it only under `-D ASSERT=all`; its default level skips
# it and prints `ééééhi`. Mojito evaluates every standard-library assert
# (docs/non-goals.md).
# expect: abort: fill char needs to be a one byte literal
def main():
    var s = String("hi")
    print(s.ascii_rjust(6, "é"))
