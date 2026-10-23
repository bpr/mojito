# A `break` out of two nested `try`s runs both `finally`s, inner first,
# each copied before the `break`; `s` dies after the inner one, its last
# reader on that path.
#
# Expected output:
#   inner 0
#   outer
#   next 0
#   drop 0
#   inner 1
#   drop 1
#   outer
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
            try:
                if i == 1:
                    break
            finally:
                print("inner", s.tag)
        finally:
            print("outer")
        print("next", s.tag)
    print("after")
