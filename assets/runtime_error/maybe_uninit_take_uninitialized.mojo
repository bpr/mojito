# Consuming unsafe_assume_init on uninitialized storage is upstream UB; the
# VM traps deterministically.
# expect: take of uninitialized MaybeUninit storage
from std.memory import MaybeUninit

def main():
    var a = MaybeUninit[Int]()
    print(a^.unsafe_assume_init())
