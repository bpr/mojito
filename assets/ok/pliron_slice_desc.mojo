# Slice-descriptor construction and consumption: contiguous slices with
# both, one, and no explicit bounds (`Optional` bound materialization
# through the compiled constructors), and a strided slice normalized by the
# intrinsic `indices`.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4, 5]
    print(xs[1:4])
    print(xs[:3])
    print(xs[2:])
    print(xs[:])
    print(xs[::2])
    print(xs[4:1:-1])
