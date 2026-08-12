# A raw pointer read out of an Allocation before dealloc diagnoses
# deterministically after the storage is released.
# expect: use after Pointer deallocation
from std.memory import Layout, dealloc

def main():
    var allocation = alloc(Layout[Int](count=1))
    var ptr = allocation.unsafe_ptr()
    ptr.unsafe_write(1)
    dealloc(allocation^)
    print(ptr[])
