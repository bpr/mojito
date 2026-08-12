from std.memory import Layout, dealloc, unsafe_alloc

def main():
    var allocation = alloc(Layout[Int](count=4))
    var ptr = allocation.unsafe_ptr()
    ptr.unsafe_offset(0).unsafe_write(42)
    print(ptr[])
    print(allocation.layout().count())
    dealloc(allocation^)
    var raw = unsafe_alloc[Int](1)
    raw.unsafe_write(7)
    print(raw[unsafe_offset=0])
    raw.unsafe_free()
