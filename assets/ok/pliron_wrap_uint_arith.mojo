# Defined wrapping mod 2^64 for UInt + - * (the shared native ABI contract,
# docs/native-abi.md): underflow and overflow are ordinary wrapped values.
def compute() -> UInt:
    var zero = UInt(0)
    var one = UInt(1)
    var max = zero - one
    var add_wrap = max + one
    var mul_wrap = max * UInt(2)
    return add_wrap + mul_wrap + max

def main():
    print(compute())
