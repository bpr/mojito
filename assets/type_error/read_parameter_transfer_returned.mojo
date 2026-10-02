# expect: cannot transfer out of immutable reference
# A parameter with no owning convention is read-only: the callee does not own
# the caller's `String`, so `x^` cannot move out of it.
def ident(x: String) -> String:
    return x^


def main():
    var s = String("s")
    print(ident(s))
    print(s)
