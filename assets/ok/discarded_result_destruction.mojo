# Pinned against Mojo 1.1.0.dev2026082605 (2026-09-08): a discarded call
# result is destroyed before the next statement, and the unbound result of
# a non-consuming `__enter__` is destroyed before the body runs.
# Expected: del tok 2 / after discard / enter / del tok 1 / body / exit / done
struct Tok:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del tok", self.n)


struct Mgr:
    def __init__(out self):
        pass

    def __enter__(self) -> Tok:
        print("enter")
        return Tok(1)

    def __exit__(self):
        print("exit")


def make() -> Tok:
    return Tok(2)


def main():
    make()
    print("after discard")
    with Mgr():
        print("body")
    print("done")
