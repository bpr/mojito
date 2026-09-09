# expect: use of uninitialized value 's'
# A value consumed by a raising call inside a `try` body is gone in the
# `except` arm: consumption happens at the call, and the raise reaches the
# handler after it (pinned Mojo a79fbdf59f2's text).
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
        print(s.x)
