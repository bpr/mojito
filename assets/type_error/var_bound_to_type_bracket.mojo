# expect: dynamic type values not permitted yet
# A type application has no runtime value in whichever bracket node it
# parses as (`pair[1, 2]` is a `MultiIndex`): it binds through `comptime`.


struct pair[a: Int, b: Int]:
    pass


def main():
    var x = pair[1, 2]
    print(1)
