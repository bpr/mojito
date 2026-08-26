# Borrowing unsafe_assume_init on uninitialized storage is upstream UB; the
# VM traps deterministically.
# expect: read of uninitialized MaybeUninit storage
from std.memory import MaybeUninit

def main():
    var a = MaybeUninit[Int]()
    print(a.unsafe_assume_init())
