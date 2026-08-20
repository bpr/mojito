def compute() -> Int:
    var flags = 0
    var big = 10000000000000000000.0
    if Int(big) == 9223372036854775807:
        flags = flags + 1
    var neg_big = -10000000000000000000.0
    if Int(neg_big) == -9223372036854775808:
        flags = flags + 2
    var zero = 0.0
    var nan = zero / zero
    if Int(nan) == 0:
        flags = flags + 4
    if Int(3.7) == 3:
        flags = flags + 8
    if Int(-3.7) == -3:
        flags = flags + 16
    if Float64(7) == 7.0:
        flags = flags + 32
    return flags

def main():
    print(compute())
