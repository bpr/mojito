# unsafe_deinit runs the payload destructor, so it requires a Deinitable
# payload; ThinAllocation is linear (Deinitable where False).
# expect: no overload matches
from std.memory import MaybeUninit, ThinAllocation

def main():
    var a = MaybeUninit[ThinAllocation[Int]]()
    a^.unsafe_deinit()
