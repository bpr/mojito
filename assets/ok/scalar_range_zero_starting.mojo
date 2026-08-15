# requires: discovery
# (The checker-inferred constructor rewrite is a Compiler-owned handoff;
# the raw verify seam skips this fixture.)
# The dtype-inferred scalar `range(end)` family: a non-Int integral scalar
# argument selects the zero-starting range over Scalar[dtype] elements
# through checker-inferred specialization (upstream's infer-only
# `range[dtype: DType, //]` overloads have no explicit spelling).
def main():
    for x in range(Int32(4)):
        print(x)
    print(len(range(UInt8(3))))
    print(range(Int16(5))[2])
    # A negative end clamps to the empty range (upstream's max(end, 0)).
    print(len(range(Int32(-3))))
