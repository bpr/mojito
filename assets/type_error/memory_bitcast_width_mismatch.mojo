# expect: the source and destination types must have the same bitwidth
from std.memory import bitcast


def main():
    print(bitcast[DType.uint16](UInt32(3)))
