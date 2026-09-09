# expect: use of uninitialized value 's'
# Moves inside a `try` body are checked like any other: the second transfer
# reads a consumed value.
@fieldwise_init
struct Thing:
    var x: Int


def take(var s: Thing) raises:
    print("took", s.x)


def main():
    var s = Thing(1)
    try:
        take(s^)
        take(s^)
    except e:
        print("caught")
