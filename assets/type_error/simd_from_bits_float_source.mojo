# `SIMD`'s `from_bits=` initializer copies an integral vector's bits; its
# `comptime assert` fails a float source, as at the pin.
# expect: constraint failed: the simd type must be integral
def main():
    print(Float32(from_bits=Float32(1.0)))
