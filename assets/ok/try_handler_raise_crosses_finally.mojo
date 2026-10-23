# A raise from an `except` past the `try`'s `finally`, out of the
# function: the copied `finally` runs before the raise, and what the
# handler and the `finally` read dies after each last reads it.
#
# Expected output:
#   finally 0
#   next 0 10
#   drop 10
#   drop 0
#   handler 11
#   drop 11
#   finally 1
#   drop 1
#   caught boom


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def g(x: Int) raises:
    if x > 0:
        raise Error("boom")


def f(i: Int) raises:
    var s = S(i)
    var t = S(10 + i)
    try:
        g(i)
    except e:
        print("handler", t.tag)
        raise e^
    finally:
        print("finally", s.tag)
    print("next", s.tag, t.tag)


def main():
    try:
        f(0)
        f(1)
    except e:
        print("caught", e)
