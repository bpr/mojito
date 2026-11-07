# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin body's fold takes every argument by value: a `mut` argument is
# rejected, as are `var`, `ref`, and `deinit`.
@always_inline("builtin")
def bump(mut n: Int):
    n += 1


def main():
    var x = 1
    bump(x)
    print(x)
