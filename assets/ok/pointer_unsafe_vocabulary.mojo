# The current unsafe_* pointer vocabulary: unsafe_write (move and copy=),
# unsafe_offset chaining, empty-subscript reads, unsafe_take_pointee,
# unsafe_deinit_pointee, and unsafe_free, plus the place-pointer write-through.
from std.memory import unsafe_alloc

def main():
    var p = unsafe_alloc[Int](2)
    p.unsafe_write(41)
    p.unsafe_offset(1).unsafe_write(1)
    print(p[] + p.unsafe_offset(1)[])
    var taken = p.unsafe_take_pointee()
    print(taken)
    p.unsafe_offset(1).unsafe_deinit_pointee()
    p.unsafe_free()
    var x = 3
    var copied = unsafe_alloc[Int](1)
    copied.unsafe_write(copy=x)
    print(x, copied[])
    copied.unsafe_free()
    var q = Pointer(to=x)
    q.unsafe_write(9)
    print(x)
