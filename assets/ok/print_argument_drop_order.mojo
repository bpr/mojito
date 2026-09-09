# Pinned against Mojo 1.1.0.dev2026082605 (2026-09-08): an owner whose last
# use is a field read feeding `print` is destroyed after the line prints.
# Expected: use 1 / del tok 1 / mid
struct Tok:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del tok", self.n)


def main():
    var t = Tok(1)
    print("use", t.n)
    print("mid")
