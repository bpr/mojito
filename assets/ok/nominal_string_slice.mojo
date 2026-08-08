# Contiguous slicing on the nominal String: non-raising byte-wise library
# code with Python-normalized bounds, matching the builtin literal slice.
def main():
    var s: String = "hello"
    print(s[1:4])
    print(s[-2:])
    print(len(s[0:0]))
