# Assigning to a `mut` pointer parameter reseats the caller's pointer: the
# parameter binds a reference to the caller's pointer slot, distinct from
# the pointer the slot holds, so `p = q` stores `q` into that slot and
# leaves `x` and `y` as they were. Upstream prints `2 1 2` and `1 7`.
def reseat(
    mut p: Pointer[Int, MutUntrackedOrigin],
    q: Pointer[Int, MutUntrackedOrigin],
):
    p = q


def main():
    var x = 1
    var y = 2
    var p = Pointer(to=x).unsafe_origin_cast[MutUntrackedOrigin]()
    var q = Pointer(to=y).unsafe_origin_cast[MutUntrackedOrigin]()
    reseat(p, q)
    print(p[], x, y)
    p[] = 7
    print(x, y)
