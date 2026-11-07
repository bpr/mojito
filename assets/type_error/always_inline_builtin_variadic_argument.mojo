# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin takes no variadic argument, called or not.
@always_inline("builtin")
def first(*n: Int) -> Int:
    return 1


def main():
    print(1)
