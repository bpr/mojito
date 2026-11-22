# `Pointer.write` requires `type_of(self).mut`: an immutable pointer
# parameter cannot write.
# expect: violated constraint
def take(p: ImmPointer[Int, _]):
    p.write(3)


def main():
    var x = 1
    take(Pointer(to=x))
