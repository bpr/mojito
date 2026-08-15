# A negative bound on a contiguous List slice aborts instead of wrapping.
# expect: abort: slice start index -2 is out of bounds, valid range is 0 to 5
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[-2:])
