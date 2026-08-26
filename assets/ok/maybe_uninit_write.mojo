# `MaybeUninit.write` (upstream 2026-08): the safe counterpart of
# `unsafe_write` for trivially-deinitable payloads — overwriting a live value
# cannot leak because the deinitializer is a no-op.
from std.memory import MaybeUninit

def main():
    var slot = MaybeUninit[Int]()
    slot.write(1)
    slot.write(2)
    print(slot.unsafe_assume_init())
