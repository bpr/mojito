# expect: the target type must be at least as wide as the source type
def main():
    print(Int64(4).to_bits[DType.uint16]())
