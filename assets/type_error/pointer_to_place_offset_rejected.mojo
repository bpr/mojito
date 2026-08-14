# expect: designates a single value
# An origin-bearing pointer to a precise place (`Pointer(to=x)`) still
# designates exactly one value: only offset 0 dereferences. The
# multi-element lift applies only to interior-generation domains such as
# `List.unsafe_ptr()`.
def main():
    var x = 42
    var p = UnsafePointer(to=x)
    print(p[1])
