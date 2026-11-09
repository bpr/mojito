# A `comptime if` whose condition the check closes decides its arms before
# the ownership checks, as upstream's `LowerSemanticCF` does: a dead arm
# reads nothing, and the code after a live arm that never falls through is
# erased, so a value nothing live reads dies right after its definition.
# The pin prints each `del` before the line that follows it in source order.
comptime A = False
comptime B = True


struct Thing(Movable):
    var s: String

    def __init__(out self, s: String):
        self.s = s

    def __del__(deinit self):
        print("del", self.s)


def live_returns():
    var b = Thing("r")
    print("r-start")
    comptime if B:
        return
    print("after", b.s)


def dead_reads():
    var b = Thing("d")
    print("d-start")
    comptime if A:
        print("dead", b.s)
    else:
        return
    print("after", b.s)


def elif_returns():
    var b = Thing("e")
    print("e-start")
    comptime if A:
        print("A", b.s)
    elif B:
        return
    else:
        print("else", b.s)
    print("after", b.s)


def live_raises() raises:
    var b = Thing("x")
    print("x-start")
    comptime if B:
        raise Error("boom")
    print("after", b.s)


def looped():
    for i in range(2):
        var b = Thing("l" + String(i))
        print("l-start", i)
        comptime if B:
            continue
        print("after", b.s)


def open_arm[flag: Bool]():
    var b = Thing("o")
    print("o-start")
    comptime if flag:
        return
    print("after", b.s)


def main():
    live_returns()
    dead_reads()
    elif_returns()
    try:
        live_raises()
    except e:
        print(e)
    looped()
    open_arm[True]()
