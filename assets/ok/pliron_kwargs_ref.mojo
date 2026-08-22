# Keyword-bound `mut` arguments resolve their checked places through the
# call-slot matcher (the place comes from the matched keyword source, never
# the parameter position), and the `Type(copy=value)` constructor form runs
# the copy constructor.
@fieldwise_init
struct Tally(Copyable):
    var n: Int

def bump(base: Int, mut sink: Tally):
    sink.n += base

def main():
    var t = Tally(5)
    bump(sink=t, base=2)
    print(t.n)
    bump(2, sink=t)
    print(t.n)
    var u = t.copy()
    u.n += 100
    print(t.n, u.n)
