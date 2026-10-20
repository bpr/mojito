# PROBE: `String(slice)` on the native backend.
#
# The pin and the VM print:
#   Slice(1, 3, None)
#   Slice(1, 3, None)
#
# The Pliron backend prints the descriptor's raw layout on the second line,
# `Slice(start=1, end=3, step=0, flags=3)` (R1158): the elaborator gives the
# intrinsic descriptor a raw four-field struct with no `write_to`.
def main():
    var s = Slice(1, 3)
    print(s)
    print(String(s))
