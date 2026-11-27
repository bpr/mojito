# expect: failed to infer parameter 'mut'
# A call binds an origin binder only from its bracket or its arguments'
# places, and no argument here names `o`: upstream fails to infer the
# implicit `mut` a bare `Origin` declares first.
def f[o: Origin](x: Int):
    print(x)


def main():
    f(1)
