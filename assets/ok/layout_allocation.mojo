# The current layout-based allocation model: alloc(Layout[T](count=n)) returns
# an Allocation[T] owning its heap storage through a ThinAllocation[T] and
# retaining the Layout used to allocate it; dealloc(allocation^) releases it.
# `alloc` is a prelude name; the rest of the vocabulary imports from std.memory.
from std.memory import Layout, ThinAllocation, dealloc

def main():
    var allocation = alloc(Layout[Int](count=4))
    var ptr = allocation.unsafe_ptr()
    ptr.unsafe_offset(0).unsafe_write(42)
    ptr.unsafe_offset(1).unsafe_write(0)
    print(ptr[])
    print(allocation.layout().count())
    dealloc(allocation^)
    var aligned = alloc(Layout[Int](count=1, alignment=16))
    aligned.unsafe_ptr().unsafe_write(7)
    print(aligned.unsafe_ptr()[])
    var thin: ThinAllocation[Int] = aligned^.into_thin()
    var raw = thin^.unsafe_leak()
    print(raw[])
    raw.unsafe_free()
