# expect: use of uninitialized value 'o'
# A raising named destructor consumes its receiver at the call, so the
# `except` arm cannot read it — for an implicitly destructible value exactly
# as for a linear one.
struct Owned:
    var n: Int

    def __init__(out self):
        self.n = 1

    def __deinit__(deinit self):
        print("del")

    def consume(deinit self) raises:
        raise Error("boom")


def main():
    var o = Owned()
    try:
        o^.consume()
    except e:
        print("caught", o.n)
