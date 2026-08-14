# A contiguous Span sub-slice with out-of-range bounds aborts instead of
# clamping.
# expect: abort: Span slice bounds out of range
def main():
    var xs: List[Int] = [1, 2, 3]
    var sp = Span(xs)
    var sub = sp[0:9]
    print(len(sub))
