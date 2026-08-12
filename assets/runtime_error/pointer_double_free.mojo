# All aliases share allocation provenance and observe deallocation.
# expect: double free
from std.memory import unsafe_alloc

def main():
    var pointer = unsafe_alloc[Int](1)
    var alias = pointer
    pointer.free()
    alias.free()
