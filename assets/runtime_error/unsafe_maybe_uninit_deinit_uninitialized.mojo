# unsafe_deinit on uninitialized storage is upstream UB; the VM traps
# deterministically.
# expect: destroy of uninitialized UnsafeMaybeUninit storage
from std.memory import UnsafeMaybeUninit

def main():
    var a = UnsafeMaybeUninit[Int]()
    a^.unsafe_deinit()
