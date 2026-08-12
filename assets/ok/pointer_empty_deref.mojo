# The empty subscript `p[]` is the direct pointer dereference: offset-0
# load/store on a heap pointer, and pointee access on a `Pointer(to=x)`
# place pointer (writes reach the owner through the handle).
from std.memory import unsafe_alloc

def main():
    var p = unsafe_alloc[Int](1)
    p[] = 41
    p[] += 1
    print(p[])
    p.free()
    var x = 5
    var q = Pointer(to=x)
    q[] += 1
    print(q[])
    print(x)
