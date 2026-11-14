# A module `comptime` value whose initializer prints is evaluated once, at
# compile time: `no` is printed while the program elaborates, and the
# program prints only `1`.
def bad() -> Int:
    print("no")
    return 1

comptime X = bad()


def main():
    print(X)
