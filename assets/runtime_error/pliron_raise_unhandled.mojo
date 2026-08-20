# expect: unhandled error: negative input
def fail(n: Int) raises -> Int:
    if n < 0:
        raise Error("negative input")
    return n * 2


def main() raises:
    print(fail(5))
    print(fail(-1))
