# A contiguous Span sub-slice with out-of-range bounds aborts instead of
# clamping.
# expect: abort: slice end index 9 is out of bounds, valid range is 0 to 3
def main():
    var xs: List[Int] = [1, 2, 3]
    var sp = Span(xs)
    var sub = sp[0:9]
    print(len(sub))
