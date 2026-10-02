# A `mut self` receiver is the caller's storage like any `mut` parameter.
# expect: 'self' is uninitialized at return from this function
# requires: stdlib
struct P:
    var a: String

    def __init__(out self):
        self.a = String("a")

    def gone(mut self) -> P:
        return self^


def main():
    var p = P()
    var q = p.gone()
    print(q.a)
