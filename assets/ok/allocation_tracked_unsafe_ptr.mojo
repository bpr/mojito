# Allocation.unsafe_ptr() is tracked: the pointer carries the Allocation's
# element interior-generation origin, so offsets and writes execute while
# the owner lives and the pointer's last use precedes the consuming
# dealloc.
from std.memory import Layout, dealloc

def main():
    var a = alloc(Layout[Int](count=3))
    var p = a.unsafe_ptr()
    p.unsafe_offset(0).unsafe_write(7)
    p.unsafe_offset(1).unsafe_write(8)
    p.unsafe_offset(2).unsafe_write(9)
    print(p[0] + p[1] + p[2])
    dealloc(a^)
