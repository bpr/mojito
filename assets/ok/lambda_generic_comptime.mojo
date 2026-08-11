# Generic lambdas invoke inline with call-site parameter binding; a lambda
# binds a callable-bound generic type parameter as a runtime argument; and a
# compile-time-unrolled loop clones a lambda per iteration, substituting the
# unroll variable into each clone's body.
def transform[F: def(x: Int) -> Int](f: F, v: Int) -> Int:
    return f(v)

def main():
    print((lambda [N: Int](x: Int) {} -> Int: x + N)[5](3))
    print(transform(lambda (x: Int) {} -> Int: x * 3, 5))
    var total = 0
    comptime for k in range(3):
        total = total + (lambda (x: Int) {} -> Int: x + k)(10)
    print(total)
