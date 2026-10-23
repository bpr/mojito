# A raise from an `else` leaves the `try` past its `finally`, which is
# copied before the raise: `s` dies after the copy reads it, before the
# error leaves the function.
#
# Expected output:
#   else 0
#   finally 0
#   after 0
#   drop 0
#   else 1
#   finally 1
#   drop 1
#   caught g1


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def g(x: Int) raises:
    if x > 0:
        raise Error("g" + String(x))


def f(i: Int) raises:
    var s = S(i)
    try:
        g(0)
    except e:
        print("unreached", e)
    else:
        print("else", s.tag)
        g(i)
    finally:
        print("finally", s.tag)
    print("after", s.tag)


def main():
    try:
        f(0)
        f(1)
    except e:
        print("caught", e)
