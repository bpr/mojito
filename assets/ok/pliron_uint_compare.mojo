def compute() -> Int:
    var big = UInt(-1)
    var small = UInt(5)
    var flags = 0
    if big > small:
        flags = flags + 1
    if small < big:
        flags = flags + 2
    if big >= UInt(-2):
        flags = flags + 4
    if small != big:
        flags = flags + 8
    return flags

def main():
    print(compute())
