# expect: function instantiation of `main` failed: constraint failed: Element type must be of integral type.
# An `IndexList` over a floating-point element type fails its constructor's
# `comptime assert`, as at the pin.
from std.utils import IndexList


def main():
    print(IndexList[2, element_type=DType.float32](fill=1))
