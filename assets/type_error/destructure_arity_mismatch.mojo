# expect: 3-element tuple
# A nested tuple target must match the element's static arity: a pair has no
# third element to destructure.
def main():
    var p = (1, (2, 3))
    var (a, (b, c, d)) = p
    print(a, b, c, d)
