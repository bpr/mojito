# Recorded divergence (2026-09-08, Mojo 1.1.0.dev2026082605): a `deinit self`
# field read only inside a loop body. The pinned Mojo destroys such a field
# at the destructor's ENTRY and the loop then reads the destroyed value's
# bits (`del inner 3 / del inner 4 / start / for 3 / for 3 / while 4 0 /
# while 4 1 / end / mid`), while a plain local read in a loop dies after
# the loop (`loop 9 / loop 9 / del inner 9`). Mojito keeps the sound order —
# the field dies after the loop (`start / for 3 / for 3 / del inner 3 /
# while 4 0 / while 4 1 / del inner 4 / end / mid`) — as an upstream bug
# deliberately not matched; re-probe at every re-pin.
struct Inner:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("del inner", self.id)


struct Outer:
    var q: Inner
    var r: Inner

    def __init__(out self):
        self.q = Inner(3)
        self.r = Inner(4)

    def __deinit__(deinit self):
        print("start")
        for i in range(2):
            print("for", self.q.id)
        var j = 0
        while j < 2:
            print("while", self.r.id, j)
            j += 1
        print("end")


def main():
    var x = Inner(9)
    var i = 0
    while i < 2:
        print("loop", x.id)
        i += 1
    print("after loop")
    var o = Outer()
    print("mid")
