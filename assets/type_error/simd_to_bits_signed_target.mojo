# expect: the target type must be unsigned integral
def main():
    print(Int8(4).to_bits[DType.int16]())
