# Assigning to a `mut` `Pointer` parameter (upstream accepts it and prints
# `5`). The parameter's slot stores the caller's pointer as its own value —
# both backends read a pointer-typed slot as the handle it holds — so the
# assignment replaces the slot rather than writing through the pointer at its
# pointee.
def write_back[o: Origin](mut p: Pointer[Int, o]):
    p = p


def main():
    var x = 5
    var q = Pointer(to=x)
    write_back(q)
    print(q[])
