# expect: is not defined for ContiguousSlice
# Only `Slice` is Equatable upstream; the `ContiguousSlice`/`StridedSlice`
# descriptor sub-kinds do not compare.
def same(a: ContiguousSlice, b: ContiguousSlice) -> Bool:
    return a == b

def main():
    print(same(Slice(1, 2), Slice(1, 2)))
