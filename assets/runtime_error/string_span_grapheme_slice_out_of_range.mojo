# A StringSpan grapheme slice past the cluster count aborts.
# expect: abort: StringSpan grapheme slice bounds out of range
def main():
    var s = String("hi")
    var sp = StringSpan(s)
    var g = sp[grapheme=0:5]
    print(g)
