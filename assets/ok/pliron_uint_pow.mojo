def compute() -> UInt:
    var b = UInt(3)
    var e = UInt(7)
    return b ** e + UInt(2) ** UInt(0)

def main():
    print(compute())
