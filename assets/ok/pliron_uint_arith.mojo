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
    total = total + (UInt(1) << UInt(70))
    total = total + (UInt(1024) >> UInt(68))
    return total

def main():
    print(compute())
