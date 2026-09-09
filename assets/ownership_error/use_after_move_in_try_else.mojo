# expect: use of uninitialized value 's'
# The `else` arm runs after the body completed, so a value the body consumed
# is uninitialized there.
@fieldwise_init
struct Thing:
    var x: Int


def take(var s: Thing) raises:
    print("took", s.x)


def main():
    var s = Thing(1)
    try:
        take(s^)
    except e:
        print("caught")
    else:
        print("else", s.x)
