# expect: designates a single value
# `p[1]` on a pointer to a precise place (`Pointer(to=x)`) runs the declared
# offset getter, `self.unsafe_offset(1)[]`, and the VM traps: a place pointer
# designates exactly one value (R2212). The pin reads the neighbouring stack
# slot. The multi-element lift applies only to interior-generation domains
# such as `List.unsafe_ptr()`.
def main():
    var x = 42
    var p = Pointer(to=x)
    print(p[1])
