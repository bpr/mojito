# expect: failed to infer parameter 'm'
# The `mut=` binder of an origin binder no argument names is the first
# parameter upstream fails to infer.
def f[m: Bool, //, o: Origin[mut=m]](x: Int):
    print(x)


def main():
    f(1)
