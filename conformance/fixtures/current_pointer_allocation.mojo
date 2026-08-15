from std.memory import Layout, alloc, dealloc

def main():
    var allocation = alloc(Layout[Int](count=4))
    var ptr = allocation.unsafe_ptr()
    ptr.unsafe_offset(0).unsafe_write(42)
    print(ptr[])
    print(allocation.layout().count())
    dealloc(allocation^)
