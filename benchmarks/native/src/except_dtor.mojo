# Benchmark: exceptions and destructor-heavy control flow. Raising functions
# inside try/except/else/finally loops, plus a struct whose __deinit__ bumps a
# shared drop counter through a pointer; only final counts are printed.
from std.memory import unsafe_alloc

struct Tracked(Copyable, Movable):
    var ctr: UnsafePointer[Int]
    var id: Int

    def __init__(out self, ctr: UnsafePointer[Int], id: Int):
        self.ctr = ctr
        self.id = id

    def __deinit__(deinit self):
        self.ctr[0] += 1

def risky(i: Int) raises -> Int:
    if i % 7 == 0:
        raise "seven"
    if i % 11 == 0:
        raise "eleven"
    return i % 5

def main():
    var drops: UnsafePointer[Int] = unsafe_alloc[Int](1)
    drops[0] = 0

    var caught: Int = 0
    var ok_sum: Int = 0
    var else_hits: Int = 0
    var finally_hits: Int = 0
    var i: Int = 0
    while i < 600000:
        try:
            ok_sum += risky(i)
        except e:
            caught += 1
        else:
            else_hits += 1
        finally:
            finally_hits += 1
        i += 1

    var id_sum: Int = 0
    var d: Int = 0
    while d < 20000:
        var t = Tracked(drops, d)
        var u = t.copy()
        id_sum += u.id
        d += 1

    print("caught:", caught)
    print("ok_sum:", ok_sum)
    print("else_hits:", else_hits)
    print("finally_hits:", finally_hits)
    print("id_sum:", id_sum)
    print("drops:", drops[0])
    drops.free()
