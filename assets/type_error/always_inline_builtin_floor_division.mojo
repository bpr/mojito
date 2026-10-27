# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# `SIMD.__floordiv__` is not builtin, so `Int` floor division is a call a
# builtin body cannot make.
@always_inline("builtin")
def half(n: Int) -> Int:
    return n // 2


def main():
    print(1)
