# expect: conflicts with live reference
# A subscript view temporary passed as an argument keeps a loan on its source
# for the statement: a later argument mutating the source conflicts with it.
def grow(mut s: String) -> Int:
    s += "zz"
    return 1


def main():
    var s = String("hello")
    var out = String()
    out.write(s[byte=1:3], grow(s))
    print(out)
