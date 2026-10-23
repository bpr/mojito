# A `break` out of a `try` whose `finally` reads `s`, which only the
# fall-through path reads after the `try`: the `finally` copied before the
# `break` runs, then `s` dies there, before the loop's exit.
#
# Expected output:
#   finally 0
#   next 0
#   drop 0
#   finally 1
#   drop 1
#   after


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def main():
    for i in range(3):
        var s = S(i)
        try:
            if i == 1:
                break
        finally:
            print("finally", s.tag)
        print("next", s.tag)
    print("after")
