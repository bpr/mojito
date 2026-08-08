# The nominal String slice is a non-raising API surface: a def without
# `raises` may slice, and out-of-range bounds clamp instead of failing.
def main():
    var s: String = "parity"
    print(s[2:100])
    print(s[-100:3])
    print(len(s[4:2]))
