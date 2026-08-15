# Out-of-range byte slice bounds abort instead of clamping, and reversed
# bounds are out of range too.
# expect: abort: slice end index 9 is out of bounds, valid range is 0 to 5
def main():
    var s = String("hello")
    var cut = s[byte=2:9]
    print(cut)
