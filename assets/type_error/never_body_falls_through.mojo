# A `-> Never` function whose body reaches its end is rejected: `Never`
# names no value to fall through with. The pin reports "return expected at
# end of function with results".
# expect: does not return a value on every path
def die(x: Int) -> Never:
    print("dying", x)


def main():
    print(1)
