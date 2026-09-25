# PROBE (divergence): unpacking a tuple whose element owns heap storage.
#
# The pinned Mojo copies each element out of the tuple and prints both.
# Mojito accepts the program and then stops at run time: the VM reports
# "use after Pointer deallocation" for the local tuple below, and
# `--backend pliron` reports "double free of Pointer allocation" for the same
# unpack of a `Tuple[String, Int]` parameter, which the VM runs.
#
# Observed 2026-09-25 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   t 6
#   mojito: run error
#
# When fixed: promote to `assets/ok`.
def main():
    var t = (String("t"), 6)
    var s, n = t
    print(s, n)
