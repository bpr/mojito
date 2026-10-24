# requires: stdlib
# The `break` out of the `try` carries the `mut` parameter moved to the
# loop's exit, where it is written back before the function returns.
def take(var s: String) raises:
    print(s)


def f(mut s: String) raises:
    for _ in range(2):
        try:
            take(s^)
            break
        except:
            s = "d"
    s = "e"


def main() raises:
    var s = String("x")
    f(s)
    print(s)
