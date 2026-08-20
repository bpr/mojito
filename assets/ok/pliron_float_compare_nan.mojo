def compute() -> Int:
    var zero = 0.0
    var nan = zero / zero
    var flags = 0
    if nan == nan:
        flags = flags + 1
    if nan != nan:
        flags = flags + 2
    if nan < 1.0:
        flags = flags + 4
    if nan >= 1.0:
        flags = flags + 8
    if 1.0 <= 2.0:
        flags = flags + 16
    return flags

def main():
    print(compute())
