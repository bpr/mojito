# An out-of-range bound on a contiguous List slice aborts instead of clamping,
# and the abort is not catchable: `try`/`except` observes only raised errors.
# expect: abort: slice end index 9 is out of bounds, valid range is 0 to 3
def main():
    var xs: List[Int] = [0, 1, 2]
    try:
        print(xs[0:9])
    except e:
        print("unreachable", e)
