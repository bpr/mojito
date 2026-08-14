# Reversed bounds on a contiguous List slice abort instead of producing an
# empty result.
# expect: abort: List slice bounds out of range
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[3:1])
