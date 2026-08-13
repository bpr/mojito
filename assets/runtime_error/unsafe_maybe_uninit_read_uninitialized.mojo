# Borrowing unsafe_assume_init on uninitialized storage is upstream UB; the
# VM traps deterministically.
# expect: read of uninitialized UnsafeMaybeUninit storage
from std.memory import UnsafeMaybeUninit

def main():
    var a = UnsafeMaybeUninit[Int]()
    print(a.unsafe_assume_init())
