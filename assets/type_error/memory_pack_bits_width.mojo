# expect: the logical bitwidth of the bool vector must be the same as the target type
from std.memory import pack_bits


def main():
    print(pack_bits[DType.uint8](SIMD[DType.bool, 16](fill=True)))
