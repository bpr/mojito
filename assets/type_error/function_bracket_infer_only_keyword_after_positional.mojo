# expect: unexpected keyword parameter 'o'
# An infer-only origin binder binds by keyword only ahead of the positional
# arguments: the first positional argument steps past the infer-only list.
def f[o: ImmOrigin, //, T: AnyType]():
    print("f")


def main():
    var x = 5
    f[Int, o=origin_of(x)]()
