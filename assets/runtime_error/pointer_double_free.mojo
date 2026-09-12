# All aliases share allocation provenance and observe deallocation.
# expect: double free
from std.memory.alloc import unsafe_alloc

def main():
    var pointer = unsafe_alloc[Int](1)
    var view = pointer
    pointer.free()
    view.free()
