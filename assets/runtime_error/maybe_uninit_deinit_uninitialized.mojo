# unsafe_deinit on uninitialized storage is upstream UB; the VM traps
# deterministically.
# expect: destroy of uninitialized MaybeUninit storage
from std.memory import MaybeUninit

def main():
    var a = MaybeUninit[Int]()
    a^.unsafe_deinit()
