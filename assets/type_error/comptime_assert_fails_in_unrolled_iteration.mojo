# expect: constraint failed: small i
# A `comptime for` judges its body's `comptime assert` once per iteration; the
# iteration where it fails fails the instance, as at the pin.

def main():
    comptime for i in range(3):
        comptime assert i < 2, "small i"
        print(i)
