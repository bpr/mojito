# expect: is not safe for VM-backed compile-time execution
def bad() -> Int:
    print("no")
    return 1

comptime X = bad()


def main():
    print(X)
