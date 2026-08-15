# A StringSpan grapheme slice past the cluster count aborts.
# expect: abort: slice end index 5 is out of bounds, valid range is 0 to 2
def main():
    var s = String("hi")
    var sp = StringSpan(s)
    var g = sp[grapheme=0:5]
    print(g)
