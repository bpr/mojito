# Strict contiguous List slicing (current Mojo bounds): in-range bounds copy,
# omitted bounds are preserved and default to the full extent, and an empty
# range is fine. Negative, out-of-range, and reversed bounds abort — pinned by
# the list_contiguous_slice_* runtime_error fixtures.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[1:3])
    print(xs[:])
    print(xs[2:])
    print(xs[:2])
    print(xs[2:2])
