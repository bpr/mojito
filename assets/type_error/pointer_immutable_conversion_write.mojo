# expect: immutable reference returned by '__getitem__'
# The capability dropped by the conversion is enforced on the result.
def main():
    var x = 5
    var r: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    r[] = 6
    print(x)
