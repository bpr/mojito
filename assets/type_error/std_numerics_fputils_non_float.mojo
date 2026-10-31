# expect: constraint failed: dtype must be floating point
# `FPUtils`' `_constraint` default is never evaluated, as at the pin, so an
# integer dtype fails at the first member that asks a float-format query.
from std.utils.numerics import FPUtils


def main():
    print(FPUtils[DType.int32].sign_mask())
