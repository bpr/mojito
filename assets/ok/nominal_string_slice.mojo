# Boundary-checked contiguous slicing on the nominal String: library code
# that raises on out-of-boundary cuts, unlike the byte-wise literal slice.
def main() raises:
    var s: String = "hello"
    print(s[1:4])
    print(s[-2:])
    print(len(s[0:0]))
