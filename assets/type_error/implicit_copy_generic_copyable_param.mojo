# expect: cannot be implicitly copied
# A `Copyable`-bounded parameter permits only explicit copies: the implicit
# `var y = x` needs an `ImplicitlyCopyable` bound or `x.copy()`.
def dup[T: Copyable](x: T) -> T:
    var y: T = x
    return y^

def main():
    print(dup(1))
