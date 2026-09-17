# expect: constraint failed: dtype must be floating point
# The floating-point format queries assert a float dtype.
def main():
    print(DType.mantissa_width[DType.int32]())
