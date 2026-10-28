# expect: Can't cast a float16 directly to a 2 x i8
from std.memory import bitcast


def main():
    print(bitcast[DType.int8, 2](Float16(1.0)))
