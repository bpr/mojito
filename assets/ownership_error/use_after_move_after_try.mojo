# expect: use of uninitialized value 's'
# Re-initializing the value in the `except` arm does not cover the normal
# path, on which the body's consumption stands: the use after the `try` is
# uninitialized on that path.
@fieldwise_init
struct Thing:
    var x: Int


def take(var s: Thing) raises:
    raise Error("boom")


def main():
    var s = Thing(1)
    try:
        take(s^)
    except e:
        s = Thing(2)
    print(s.x)
