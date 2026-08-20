def compute() -> Int:
    var flags = 0
    if Bool(3):
        flags = flags + 1
    if not Bool(0):
        flags = flags + 2
    var zero = 0.0
    if Bool(zero / zero):
        flags = flags + 4
    if not Bool(0.0):
        flags = flags + 8
    var n = 7
    if Bool(n):
        flags = flags + 16
    if Int(True) == 1:
        flags = flags + 32
    if Float64(False) == 0.0:
        flags = flags + 64
    return flags

def main():
    print(compute())
