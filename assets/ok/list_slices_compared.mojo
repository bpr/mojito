# `!=` between two mutable slices of one list reads neither span's nested
# origins (`Span.__ne__`, as upstream marks it), so the two views passed to
# it do not alias.
def main():
    var xs: List[Int] = [1, 2, 3]
    print(xs[0:2] != xs[1:3], xs[0:2] != xs[0:2])
