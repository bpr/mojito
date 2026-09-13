# Unsigned arithmetic, natively and on the VM. Shifts stay inside the word:
# Mojito masks an over-wide shift amount (`& 63`) where the pin's shift is
# poison, which the `defined-shift-overflow` conformance case carries.
def compute() -> UInt:
    var a = UInt(22)
    var b = UInt(5)
    var total = a + b
    total = total * UInt(3)
    total = total - UInt(1)
    total = total // UInt(3)
    total = total + a % b
    total = total + (a & b)
    total = total + (a | b)
    total = total + (a ^ b)
    total = total + (UInt(1) << UInt(6))
    total = total + (UInt(1024) >> UInt(4))
    return total

def main():
    print(compute())
