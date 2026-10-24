# expect: use of uninitialized value 's'
# requires: stdlib
# The `continue` out of the `try` carries `s` moved into the next iteration.
def take(var s: String) raises:
    print(s)


def main() raises:
    var s = String("x")
    for _ in range(2):
        try:
            take(s^)
            continue
        except:
            pass
        s = "y"
    print(s)
