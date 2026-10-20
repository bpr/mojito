# expect: 'comptime assert' must be inside a function
# Upstream allows a `comptime assert` only in a function body, not among a
# struct's members.

struct S[n: Int]:
    comptime assert Self.n > 0, "positive"

    def __init__(out self):
        pass


def main():
    _ = S[1]()
    print("ok")
