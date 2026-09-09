# expect: use of uninitialized value 's'
# Re-initializing the value in the `except` arm does not cover the normal
# path, on which the body's consumption stands: the use after the `try` is
# uninitialized on that path.
def take(var s: String) raises:
    raise Error("boom")


def main():
    var s = String("x")
    try:
        take(s^)
    except e:
        s = String("y")
    print(s)
