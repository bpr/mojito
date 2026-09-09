# expect: use of uninitialized value 's'
# Moves inside a `try` body are checked like any other: the second transfer
# reads a consumed value.
def take(var s: String) raises:
    print("took", s)


def main():
    var s = String("x")
    try:
        take(s^)
        take(s^)
    except e:
        print("caught")
