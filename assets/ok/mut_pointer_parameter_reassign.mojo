# Assigning to a `mut` `Pointer` parameter (upstream accepts it and prints
# `5`). The parameter's slot holds a reference to the caller's pointer slot,
# so the assignment stores through it into that slot, never at the pointee.
def write_back[o: Origin](mut p: Pointer[Int, o]):
    p = p


def main():
    var x = 5
    var q = Pointer(to=x)
    write_back(q)
    print(q[])
