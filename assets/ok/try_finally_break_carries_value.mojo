# A `break` out of a `try` whose `finally` runs before the loop's exit, after
# the body stored the value read after the loop: the value waits at the
# `break` while the `finally` runs, then enters the block after the loop.
#
# Expected output:
#   finally 0
#   finally 1
#   finally 2
#   finally 3
#   30
#   finally 0
#   finally 1
#   1


def scan(n: Int) raises -> Int:
    var last = 0
    for i in range(n):
        try:
            if i == 3:
                last = i * 10
                break
            last = i
        finally:
            print("finally", i)
    return last


def main() raises:
    print(scan(6))
    print(scan(2))
