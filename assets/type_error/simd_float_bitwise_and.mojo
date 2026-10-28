# expect: constraint failed: must be an integral or bool type
# `SIMD.__and__`'s `comptime assert` rejects a float operand in the instance
# a run-time `&` reaches.
def main():
    var f = Float64(1.5)
    print(f & f)
