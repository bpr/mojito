# expect: unexpected dtype in _integral_type_of
def main():
    print(Scalar[DType.bool](True)._to_bits_signed())
