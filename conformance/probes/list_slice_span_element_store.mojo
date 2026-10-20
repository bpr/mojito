# PROBE: an element store while a contiguous `List` slice is live.
#
# The pin runs it and prints:
#   [9, 1, 2, 3, 4]
#
# Mojito rejects it: "access to 'xs' conflicts with live reference 'v'"
# (R1160). The slice is a `Span` over the list's owned elements, and an
# element store does not move them.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    var v = xs[:]
    xs[0] = 9
    print(v)
