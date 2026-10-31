# expect: type mismatch for value parameter 'c': expected Span[Int], found Span[Float64]
def g[c: Span[Int, _]]():
    print(len(c))


def main():
    comptime c = List([Float64(1.0), 2.0, 3.0])
    g[c[:2]]()
