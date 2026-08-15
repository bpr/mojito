# requires: discovery
# (The checker-inferred constructor rewrite is a Compiler-owned handoff;
# the raw verify seam skips this fixture.)
# The three-argument scalar range: stride semantics, O(1) indexing, length,
# and the zero-step canonical empty range, all at a non-Int dtype.
def main():
    for x in range(Int32(10), Int32(0), Int32(-3)):
        print(x)
    var evens = range(Int16(2), Int16(9), Int16(2))
    print(len(evens))
    print(evens[1])
    print(len(range(Int32(1), Int32(5), Int32(0))))
