# expect: '@always_inline("builtin")' can only handle single-result if
# A builtin body takes the conditional expression, not a statement `if`.
@always_inline("builtin")
def clamp(n: Int) -> Int:
    var m = n
    if m > 3:
        m = 3
    return m


def main():
    print(1)
