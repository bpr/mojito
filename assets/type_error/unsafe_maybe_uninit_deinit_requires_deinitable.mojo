# unsafe_deinit runs the payload destructor, so it requires a Deinitable
# payload; ThinAllocation is linear (Deinitable where False).
# expect: no overload matches
from std.memory import UnsafeMaybeUninit, ThinAllocation

def main():
    var a = UnsafeMaybeUninit[ThinAllocation[Int]]()
    a^.unsafe_deinit()
