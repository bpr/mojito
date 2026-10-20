# PROBE: a `None` slice bound or step is an absent one.
#
# The pin runs it and prints:
#   [1, 2, 3, 4]
#   [0, 1, 2]
#
# Mojito rejects `xs[None:3]` at parse time ("a slice bound must be an
# expression") and `xs[1:5:None]` in the check ("type mismatch for slice
# bound: expected Int, found None") (R1159).
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[1:5:None])
    print(xs[None:3])
