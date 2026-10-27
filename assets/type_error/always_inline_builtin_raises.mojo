# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin callable does not raise.
@always_inline("builtin")
def twice(n: Int) raises -> Int:
    return n + n


def main():
    print(1)
