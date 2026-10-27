# expect: '@always_inline("builtin")' could not resolve operand value
# A builtin body's `var` is bound once; a later assignment leaves its value
# unresolved.
@always_inline("builtin")
def bumped(n: Int) -> Int:
    var m = n
    m = m + 1
    m += 2
    return m


def main():
    print(1)
