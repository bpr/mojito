# A `return` in a `finally` replaces the `return` it is copied before: the
# value the replaced `return` carried is discarded, dying at its
# definition, before the `finally` runs.
#
# Expected output:
#   drop 1
#   fin
#   got 2
#   drop 2


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def f() -> S:
    try:
        return S(1)
    finally:
        print("fin")
        return S(2)


def main():
    var s = f()
    print("got", s.tag)
