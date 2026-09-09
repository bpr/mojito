# expect: use of uninitialized value 's'
# The `else` arm runs after the body completed, so a value the body consumed
# is uninitialized there.
def take(var s: String) raises:
    print("took", s)


def main():
    var s = String("x")
    try:
        take(s^)
    except e:
        print("caught")
    else:
        print("else", s)
