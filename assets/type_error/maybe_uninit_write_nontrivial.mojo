# expect: no overload matches the supplied arguments
# `write` is gated on `IsTriviallyDeinitable[T]` (upstream: "violated
# constraint"): a non-trivially-deinitable payload must use `unsafe_write`
# after explicitly deinitializing any previous value.
from std.memory import MaybeUninit

def main():
    var slot = MaybeUninit[String]()
    slot.write(String("boxed"))
