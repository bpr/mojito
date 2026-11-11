# expect: use of uninitialized value 's'
# A variadic argument is read when its call's pack is built, so a local
# moved before the call is used uninitialized, whether the pack is
# homogeneous (`*xs: String`) or heterogeneous (`print`'s `*values`).
def f(*xs: String):
    pass

def main():
    var s = String("hi")
    var t = s^
    print(t)
    f(s)
