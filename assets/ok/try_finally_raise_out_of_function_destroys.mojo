# A raise out of a `try` without an `except` leaves the function through
# the handler MIR lowering synthesizes, which runs the copied `finally`
# and destroys `s` before the error leaves.
#
# Expected output:
#   finally 0
#   next 0
#   drop 0
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
    try:
        g(i)
    finally:
        print("finally", s.tag)
    print("next", s.tag)


def main():
    try:
        f(0)
        f(1)
    except e:
        print("caught", e)
