# Out-of-range byte slice bounds abort instead of clamping, and reversed
# bounds are out of range too.
# expect: abort: String byte slice bounds out of range
def main():
    var s = String("hello")
    var cut = s[byte=2:9]
    print(cut)
