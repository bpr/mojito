# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# A builtin body calls only builtin callables.
def thrice(n: Int) -> Int:
    return n * 3


@always_inline("builtin")
def wrapped(n: Int) -> Int:
    return thrice(n)


def main():
    print(1)
