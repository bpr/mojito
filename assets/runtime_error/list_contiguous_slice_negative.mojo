# A negative bound on a contiguous List slice aborts instead of wrapping.
# expect: abort: List slice bounds out of range
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[-2:])
