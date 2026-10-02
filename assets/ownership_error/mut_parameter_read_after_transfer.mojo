# A `mut` parameter transferred away is uninitialized until it is written
# back, so a read in between is rejected.
# expect: use of uninitialized value 'x'
# requires: stdlib
def take(mut x: String) -> String:
    var r = x^
    print(x)
    x = String("n")
    return r^


def main():
    var s = String("a")
    print(take(s))
