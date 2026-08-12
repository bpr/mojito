# expect: use after
from std.memory import unsafe_alloc

def main():
    var pointer = unsafe_alloc[Int](1)
    var alias = pointer
    pointer.free()
    print(alias[0])
