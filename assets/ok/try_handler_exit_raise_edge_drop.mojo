# A raise in a `try` body destroys, on its way to the handler, what the
# handler does not read: here the handler leaves by `break` or `return`, so
# the value the code after the `try` would have read dies before the handler
# runs, as the entry destructors of upstream's error arm destroy it.
#
# Expected output:
#   ok 10
#   drop 10
#   drop 11
#   caught 1
#   after loop
#   drop 7
#   returning
#   end
struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def may(i: Int) raises:
    if i == 1:
        raise Error("boom")


def leave_by_break():
    for i in range(3):
        var h = S(10 + i)
        try:
            may(i)
        except:
            print("caught", i)
            break
        print("ok", h.tag)
    print("after loop")


def leave_by_return() -> Int:
    var big = S(7)
    try:
        may(1)
    except:
        print("returning")
        return 1
    print("use", big.tag)
    return 0


def main():
    leave_by_break()
    _ = leave_by_return()
    print("end")
