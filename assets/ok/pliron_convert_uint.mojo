def compute() -> Int:
    var flags = 0
    var neg = -1
    if UInt(neg) == UInt(-1):
        flags = flags + 1
    var f = -1.5
    if UInt(f) == UInt(0):
        flags = flags + 2
    var big = UInt(-1)
    if Float64(big) > 9000000000000000000.0:
        flags = flags + 4
    if Int(big) == -1:
        flags = flags + 8
    var t = True
    if UInt(t) == UInt(1):
        flags = flags + 16
    return flags

def main():
    print(compute())
