# `Pointer.write` is upstream's declared custom-self method: it overwrites a
# trivially deinitable pointee through a mutable pointer, converting the
# value to the pointee type `U` the receiver binds.
from std.memory import alloc, Layout


def main():
    var a = alloc(Layout[Int](count=1))
    var p = a.unsafe_ptr()
    p.write(41)
    p.write(42)
    print(p[])
    var x = 5
    var r = Pointer(to=x)
    r.write(7)
    print(x)
    var b = alloc(Layout[UInt8](count=1))
    var q = b.unsafe_ptr()
    q.write(65)
    print(q[])
    var f = Float64(0)
    Pointer(to=f).write(2)
    print(f)
    _ = a^.unsafe_leak()
    _ = b^.unsafe_leak()
