# A `mut` parameter transferred on one path is still empty at that path's
# return, whatever the other path does.
# expect: 'x' is uninitialized at return from this function
# requires: stdlib
def take(mut x: String, c: Bool) -> String:
    if c:
        return x^
    return String("n")


def main():
    var s = String("a")
    print(take(s, False))
