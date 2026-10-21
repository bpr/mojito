# expect: Undefined variable '_'
# A `_` loop target discards each element; it declares nothing to read.
def main():
    for _ in range(2):
        print(_)
